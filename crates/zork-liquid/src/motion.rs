//! Presentation state. Hosts provide elapsed time and native visibility.
use crate::{recipes, Constraints, Material, Options, Pose, Simulation, Spring};

/// Content visibility advances independently of material deformation. Hosts
/// retain the original layout and apply this value only during composition.
#[derive(Clone, Copy)]
pub struct Reveal {
    spring: Spring,
}
impl Default for Reveal {
    fn default() -> Self {
        Self {
            spring: Spring::new(0.),
        }
    }
}
impl Reveal {
    pub fn advance(&mut self, open: bool, elapsed: f64, reduced: bool) -> bool {
        self.advance_with_frequency(open, elapsed, reduced, 28.)
    }
    fn advance_with_frequency(
        &mut self,
        open: bool,
        elapsed: f64,
        reduced: bool,
        frequency: f64,
    ) -> bool {
        self.spring.target = if open { 1. } else { 0. };
        if reduced {
            self.spring.snap();
        } else {
            self.spring.step(
                if elapsed.is_finite() {
                    elapsed.clamp(0., 0.05)
                } else {
                    0.
                },
                frequency,
                1.,
            );
        }
        if self.spring.near(0.001, 0.01) {
            self.spring.snap();
            false
        } else {
            true
        }
    }
    pub fn opacity(&self) -> f32 {
        self.spring.position.clamp(0., 1.) as f32
    }
}

/// A visibility layer waits for its entry cue once, then remains
/// visible through content/layout updates until the component is closed.
#[derive(Clone, Copy, Default)]
pub struct DelayedReveal {
    reveal: Reveal,
    ready: bool,
}
impl DelayedReveal {
    pub fn advance(&mut self, open: bool, ready: bool, elapsed: f64, reduced: bool) -> bool {
        self.ready = open && (self.ready || ready || reduced);
        // Clear the page veil before the returning material reaches its
        // source. Entry is a softer fade beginning during material expansion.
        self.reveal.advance_with_frequency(
            self.ready,
            elapsed,
            reduced,
            if self.ready { 28. } else { 84. },
        )
    }
    pub fn opacity(&self) -> f32 {
        self.reveal.opacity()
    }
}

#[derive(Default)]
pub struct Press {
    pub down: bool,
    pub release_pending: bool,
}
impl Press {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn set(
        &mut self,
        simulation: &mut Simulation,
        rest: Pose,
        down: bool,
        point: Option<[f64; 2]>,
        tap: bool,
    ) {
        if self.down == down {
            return;
        }
        self.down = down;
        if down {
            self.release_pending = false;
            let point = point.unwrap_or([0.5, 0.5]).map(|p| p.clamp(0., 1.));
            simulation.set_anchor(point, Constraints::default());
            simulation.set_target_with_speed(recipes::pressed_pose(rest, point), 1.65);
        } else if tap && rest.h - simulation.pose().h < recipes::press_depth(rest) {
            self.release_pending = true;
        } else {
            self.release_pending = false;
            simulation.set_target_with_speed(rest, 1.3);
        }
    }
    pub fn advance(
        &mut self,
        simulation: &mut Simulation,
        rest: Pose,
        elapsed: f64,
        reduced: bool,
    ) -> bool {
        if reduced && self.release_pending {
            self.release_pending = false;
            simulation.set_target(rest);
        }
        if simulation.moving() {
            simulation.advance(elapsed, reduced);
        }
        if self.release_pending && rest.h - simulation.pose().h >= recipes::press_depth(rest) {
            self.release_pending = false;
            simulation.set_target_with_speed(rest, 1.3);
        }
        simulation.moving() || self.release_pending
    }
}

#[derive(Default)]
pub struct Transition {
    retain_layout: bool,
    travel: bool,
    progress: Option<Spring>,
    material: Option<Material>,
    pub layout: Option<(Pose, Pose, bool)>,
}
impl Transition {
    /// A persistent decoration may fade out, but changing its target always
    /// continues from the current geometry instead of starting a new overlay.
    pub fn persistent() -> Self {
        Self {
            retain_layout: true,
            travel: true,
            ..Self::default()
        }
    }
    pub fn travelling() -> Self {
        Self {
            travel: true,
            ..Self::default()
        }
    }
    pub fn simulation(
        from: Pose,
        to: Pose,
        pair: bool,
        material: Material,
        anchor: [f64; 2],
    ) -> Simulation {
        let options = Options {
            capacity: (from.w * from.h).max(to.w * to.h),
            anchor,
            ..Options::default()
        };
        let mut simulation = if pair {
            Simulation::pair(from, to, material, options)
        } else {
            Simulation::new(from, material, options)
        };
        simulation.finish();
        simulation
    }
    pub fn target(
        &mut self,
        simulation: &mut Simulation,
        from: Pose,
        to: Pose,
        pair: bool,
        open: bool,
        material: Material,
        _visible: bool,
    ) {
        self.progress.get_or_insert_with(|| Spring::new(0.));
        if self.material != Some(material) {
            simulation.configure(material);
            self.material = Some(material);
        }
        if self.layout != Some((from, to, open)) {
            let dormant = !self.retain_layout
                && self.layout.is_some_and(|(_, _, was_open)| !was_open)
                && self.progress() == 0.
                && !simulation.moving();
            // A retired overlay follows layout without re-entering its exit
            // lifetime. Visibility cannot turn scrolling into a new transition.
            if dormant && open && self.layout.is_some_and(|(previous, _, _)| previous != from) {
                if pair {
                    simulation.layout_pair(from, to);
                    simulation.set_open(false);
                } else {
                    simulation.set_target(from);
                }
                simulation.finish();
            }
            if pair {
                simulation.layout_pair(from, to);
                simulation.set_open(open);
            } else {
                let target = if open { to } else { from };
                if self.travel
                    && (self.retain_layout
                        || open && self.layout.is_some_and(|(_, _, was_open)| was_open))
                {
                    simulation.set_travel_target(target);
                } else {
                    simulation.set_target(target);
                }
            }
            if dormant && !open {
                simulation.finish();
            }
            self.layout = Some((from, to, open));
        }
        self.progress.as_mut().unwrap().target = if open { 1. } else { 0. };
    }
    pub fn advance(&mut self, simulation: &mut Simulation, elapsed: f64, reduced: bool) -> bool {
        let elapsed = if elapsed.is_finite() {
            elapsed.clamp(0., 0.05)
        } else {
            0.
        };
        if simulation.moving() {
            simulation.advance(elapsed, reduced);
        }
        if let Some(progress) = &mut self.progress {
            if reduced {
                progress.snap();
            } else if !progress.near(0.001, 0.01) {
                progress.step(elapsed, simulation.omega(), 0.84);
                if progress.near(0.001, 0.01) {
                    progress.snap();
                }
            }
        }
        self.moving(simulation)
    }
    pub fn moving(&self, simulation: &Simulation) -> bool {
        simulation.moving() || self.progress.is_some_and(|p| !p.near(0.001, 0.01))
    }
    pub fn progress(&self) -> f64 {
        self.progress.map_or(0., |p| p.position.clamp(0., 1.))
    }
    /// Progress through the actual rectangle expansion, including translation.
    /// Visibility springs and numerical particle tails do not determine it.
    pub fn expansion(&self, simulation: &Simulation) -> f64 {
        let Some((from, to, _)) = self.layout else { return 0.; };
        let pose = simulation.pose();
        let values = |p: Pose| [p.left(), p.top(), p.left() + p.w, p.top() + p.h];
        let (start, target, current) = (values(from), values(to), values(pose));
        let distance = (0..4).map(|i| (target[i] - start[i]).powi(2)).sum::<f64>();
        if distance <= f64::EPSILON { return 1.; }
        ((0..4).map(|i| (current[i] - start[i]) * (target[i] - start[i])).sum::<f64>() / distance).clamp(0., 1.)
    }
    pub fn alive(&self, simulation: &Simulation) -> bool {
        self.progress() > 0.001 || simulation.moving()
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn backdrop_waits_for_its_cue_and_preserves_opacity_on_reversal() {
        let mut backdrop = DelayedReveal::default();
        for _ in 0..240 {
            backdrop.advance(true, false, 1. / 60., false);
            assert_eq!(
                backdrop.opacity(),
                0.,
                "background appeared before its entry cue"
            );
        }
        backdrop.advance(true, true, 0., false);
        for _ in 0..120 {
            backdrop.advance(true, false, 1. / 60., false);
        }
        assert_eq!(
            backdrop.opacity(),
            1.,
            "content changes must not restart the background"
        );
        backdrop.advance(false, false, 0., false);
        assert_eq!(
            backdrop.opacity(),
            1.,
            "closing must start at the current opacity"
        );
        backdrop.advance(false, false, 1. / 60., false);
        let closing = backdrop.opacity();
        assert!(closing > 0. && closing < 1.);
        backdrop.advance(true, false, 0., false);
        assert_eq!(backdrop.opacity(), closing, "reversal reset opacity");
        backdrop.advance(true, false, 1. / 60., false);
        assert!(
            backdrop.opacity() <= closing,
            "reopened before its material was ready"
        );
        backdrop.advance(true, false, 0., true);
        assert_eq!(backdrop.opacity(), 1.);
        backdrop.advance(false, false, 0., true);
        assert_eq!(backdrop.opacity(), 0.);
    }

    #[test]
    fn retired_overlay_tracks_layout_without_starting_an_exit() {
        for pair in [false, true] {
            let source = Pose::rect(20., 50., 100., 32., 16.);
            let target = Pose::rect(160., 200., 360., 280., 32.);
            let material = Material::default();
            let mut transition = Transition::default();
            let mut simulation = Transition::simulation(source, target, pair, material, [0.5, 0.5]);
            transition.target(&mut simulation, source, target, pair, true, material, true);
            transition.advance(&mut simulation, 0., true);
            transition.target(&mut simulation, source, target, pair, false, material, true);
            transition.advance(&mut simulation, 0., true);
            assert!(!transition.alive(&simulation));
            for visible in [true, false, true] {
                let relocated = Pose::rect(20., -240., 100., 32., 16.);
                transition.target(
                    &mut simulation,
                    relocated,
                    target,
                    pair,
                    false,
                    material,
                    visible,
                );
                assert!(!transition.alive(&simulation));
                assert_eq!(simulation.pose(), relocated);
            }
            let reentered = Pose::rect(20., 400., 100., 32., 16.);
            transition.target(
                &mut simulation,
                reentered,
                target,
                pair,
                true,
                material,
                true,
            );
            assert_eq!(simulation.pose(), reentered);
            assert!(transition.moving(&simulation));
        }
    }

    #[test]
    fn persistent_hover_moves_between_targets_without_becoming_a_retired_overlay() {
        let first = Pose::rect(6., 6., 180., 32., 14.);
        let next = Pose::rect(6., 70., 180., 32., 14.);
        let material = Material::default();
        let mut transition = Transition::persistent();
        let mut simulation = Transition::simulation(first, first, false, material, [0., 0.]);
        transition.target(&mut simulation, first, first, false, false, material, true);
        transition.advance(&mut simulation, 0., true);
        transition.target(&mut simulation, next, next, false, false, material, true);
        assert_eq!(simulation.pose(), first);
        assert!(transition.moving(&simulation));
        for _ in 0..8 {
            transition.advance(&mut simulation, 1. / 240., false);
        }
        assert!(simulation.pose().cy > first.cy && simulation.pose().cy < next.cy);
        let middle = simulation.pose();
        transition.target(&mut simulation, first, first, false, false, material, true);
        assert_eq!(simulation.pose(), middle);
    }

    #[test]
    fn transient_exit_keeps_material_until_fusion_finishes() {
        let source = Pose::rect(0., 0., 112., 32., 16.);
        let target = Pose::rect(0., 40., 280., 225., 32.);
        let material = Material::default();
        let mut transition = Transition::default();
        let mut simulation = Transition::simulation(source, target, false, material, [0., 0.]);
        transition.target(&mut simulation, source, target, false, true, material, true);
        for _ in 0..240 {
            transition.advance(&mut simulation, 1. / 240., false);
        }
        transition.target(
            &mut simulation,
            source,
            target,
            false,
            false,
            material,
            true,
        );
        for _ in 0..1440 {
            transition.advance(&mut simulation, 1. / 240., false);
            if !transition.alive(&simulation) {
                assert_eq!(transition.progress(), 0.);
                assert!(!simulation.moving());
                return;
            }
        }
        panic!("The material did not finish returning to its source");
    }

    #[test]
    fn dismissed_fixed_panel_relocates_without_starting_another_exit() {
        let first = Pose::rect(10., 20., 180., 120., 32.);
        let moved = Pose::rect(10., -180., 180., 120., 32.);
        let material = Material::default();
        let mut transition = Transition::default();
        let mut simulation = Transition::simulation(first, first, false, material, [0., 0.]);
        transition.target(&mut simulation, first, first, false, true, material, true);
        transition.advance(&mut simulation, 0., true);
        transition.target(&mut simulation, first, first, false, false, material, true);
        transition.advance(&mut simulation, 0., true);
        transition.target(&mut simulation, moved, moved, false, false, material, true);
        assert_eq!(simulation.pose(), moved);
        assert!(!transition.alive(&simulation));
    }

    #[test]
    fn menu_retarget_and_reverse_keep_the_presented_pose() {
        let from = Pose::rect(0., 38., 172., 2., 1.);
        let first = Pose::rect(0., 38., 172., 200., 20.);
        let next = Pose::rect(80., 38., 220., 140., 20.);
        let material = Material::default();
        let mut transition = Transition::default();
        let mut simulation = Transition::simulation(from, first, false, material, [0., 0.]);
        transition.target(&mut simulation, from, first, false, true, material, true);
        transition.advance(&mut simulation, 0., true);
        transition.target(&mut simulation, from, next, false, true, material, true);
        assert_eq!(simulation.pose(), first);
        for _ in 0..8 {
            transition.advance(&mut simulation, 1. / 240., false);
        }
        let middle = simulation.pose();
        assert!(middle.cx > first.cx && middle.cx < next.cx);
        transition.target(&mut simulation, from, first, false, true, material, true);
        assert_eq!(simulation.pose(), middle);
        for _ in 0..1440 {
            transition.advance(&mut simulation, 1. / 240., false);
        }
        assert!(!transition.moving(&simulation));
        let settled = simulation.pose();
        assert!((settled.cx - first.cx).abs() < 1e-6 && (settled.cy - first.cy).abs() < 1e-6);
        assert!((settled.w - first.w).abs() < 1e-6 && (settled.h - first.h).abs() < 1e-6);
    }
}
