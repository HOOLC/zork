//! GPUI clock and scheduling adapter for shared numerical motion.
use super::{Material, Pose, Surface};
use gpui::*;
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
use std::{cell::Cell, rc::Rc};
#[cfg(target_family = "wasm")]
use web_time::Instant;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameSample {
    pub work_ms: f64,
    pub physics_ms: f64,
    pub contour_ms: f64,
    pub controls: usize,
    pub particles: usize,
    pub moving: bool,
    pub sampled: usize,
    pub full_grid: usize,
    pub fallbacks: usize,
}

#[derive(Default)]
pub(super) struct Motion {
    pub(super) surface: Option<Surface>,
    pub(super) paint_only: bool,
    pub(super) anchor: [f64; 2],
    pub(super) state: zork_liquid::motion::Transition,
    pub(super) last: Option<Instant>,
    pub(super) scheduled: Rc<Cell<bool>>,
    pub(super) work_ms: f64,
    pub(super) physics_ms: f64,
    pub(super) samples: Vec<FrameSample>,
}

pub(super) fn notify(owner: &AnyWeakEntity, cx: &mut App) {
    if let Some(owner) = owner.upgrade() {
        cx.notify(owner.entity_id());
    }
}

pub(super) fn schedule(scheduled: &Rc<Cell<bool>>, owner: &AnyWeakEntity, window: &mut Window) {
    if !scheduled.replace(true) {
        let scheduled = scheduled.clone();
        let owner = owner.clone();
        window.on_next_frame(move |_, cx| {
            scheduled.set(false);
            notify(&owner, cx);
        });
    }
}

impl Motion {
    pub(super) fn translate(&mut self, offset: Point<Pixels>) {
        let delta = [offset.x.as_f32() as f64, offset.y.as_f32() as f64];
        if let Some(surface) = &mut self.surface {
            surface.simulation.translate(delta);
            surface.prepare();
        }
        if let Some((from, to, _)) = &mut self.state.layout {
            for pose in [from, to] {
                pose.cx += delta[0];
                pose.cy += delta[1];
            }
        }
    }
    pub(super) fn travelling() -> Self {
        Self {
            state: zork_liquid::motion::Transition::travelling(),
            ..Self::default()
        }
    }
    pub(super) fn persistent() -> Self {
        Self {
            state: zork_liquid::motion::Transition::persistent(),
            ..Self::default()
        }
    }
    pub(super) fn frame<V: 'static>(
        &mut self,
        from: Pose,
        to: Pose,
        pair: bool,
        open: bool,
        material: Material,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        self.frame_for_owner(
            from,
            to,
            pair,
            open,
            material,
            visible,
            &cx.entity().into_any().downgrade(),
            window,
            cx,
        );
    }

    pub(super) fn frame_for_owner(
        &mut self,
        from: Pose,
        to: Pose,
        pair: bool,
        open: bool,
        material: Material,
        visible: bool,
        owner: &AnyWeakEntity,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (moving, was_moving) =
            self.advance_frame(from, to, pair, open, material, visible, window, cx);
        if self.paint_only {
            return;
        }
        // One final owner render restores a trigger that was built before the
        // last simulation step retired the surface in this render pass.
        if moving || (!open && was_moving) {
            schedule(&self.scheduled, owner, window);
        }
    }

    pub(super) fn advance_frame(
        &mut self,
        from: Pose,
        to: Pose,
        pair: bool,
        open: bool,
        material: Material,
        visible: bool,
        window: &Window,
        cx: &App,
    ) -> (bool, bool) {
        let started = Instant::now();
        if self.surface.is_none() {
            let simulation =
                zork_liquid::motion::Transition::simulation(from, to, pair, material, self.anchor);
            self.surface = Some(Surface::new(simulation).expect("valid overlay material"));
        }
        let surface = self.surface.as_mut().unwrap();
        self.state.target(
            &mut surface.simulation,
            from,
            to,
            pair,
            open,
            material,
            visible,
        );
        if !InteractionGate::visible_in(window.interaction_gates().as_deref()) {
            self.state.advance(&mut surface.simulation, 0., true);
            self.physics_ms = started.elapsed().as_secs_f64() * 1000.;
            surface.prepare();
            self.work_ms = started.elapsed().as_secs_f64() * 1000.;
            self.last = None;
            return (false, false);
        }
        if !visible {
            self.last = None;
            return (false, false);
        }
        let was_moving = self.state.moving(&surface.simulation);
        let now = Instant::now();
        let dt = self
            .last
            .replace(now)
            .map_or(0., |last| now.duration_since(last).as_secs_f64())
            .min(0.05)
            * super::press::playback_rate(cx);
        let moving = self
            .state
            .advance(&mut surface.simulation, dt, cx.reduce_motion());
        let physics_ms = started.elapsed().as_secs_f64() * 1000.;
        self.physics_ms = physics_ms;
        surface.prepare();
        self.work_ms = started.elapsed().as_secs_f64() * 1000.;
        if dt > 0. || moving || self.samples.is_empty() {
            self.samples.push(FrameSample {
                work_ms: self.work_ms,
                physics_ms,
                contour_ms: self.work_ms - physics_ms,
                controls: 1,
                particles: surface.simulation.particles().count(),
                moving,
                sampled: surface.contour().sampled_points,
                full_grid: surface.contour().full_grid_points,
                fallbacks: usize::from(surface.contour().used_fallback),
            });
            if self.samples.len() > 3000 {
                self.samples.drain(..1000);
            }
        }
        if !moving {
            self.last = None;
        }
        (moving, was_moving)
    }
    pub(super) fn progress(&self) -> f64 {
        self.state.progress()
    }
    pub(super) fn expansion(&self) -> f64 {
        self.surface
            .as_ref()
            .map_or(0., |surface| self.state.expansion(&surface.simulation))
    }
    pub(super) fn alive(&self) -> bool {
        self.progress() > 0.001 || self.surface.as_ref().is_some_and(|s| s.simulation.moving())
    }
    pub(super) fn inspect(&self) -> serde_json::Value {
        self.surface.as_ref().map_or(serde_json::Value::Null, |s| serde_json::json!({
            "pose":s.simulation.pose(), "source":s.simulation.source_pose(), "progress":self.progress(), "expansion":self.expansion(),
            "open":self.state.layout.is_some_and(|(_, _, open)| open),
            "moving":s.simulation.moving(), "particles":s.simulation.particles().count(),
            "path":s.contour().svg_path(), "revision":s.contour().revision, "error":s.last_error.map(|e|format!("{e:?}")),
            "paintError":s.paint_failed(), "visible":s.visible(), "workMs":self.work_ms
        }))
    }
}
