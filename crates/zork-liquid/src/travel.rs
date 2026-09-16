//! Distance-driven translation for persistent feedback and panel transfers.
//! Shape springs still own deformation; this driver owns only the centre.
use crate::Point;

#[derive(Clone, Debug)]
pub(crate) struct Travel {
    pub position: Point,
    pub velocity: Point,
    target: Point,
    speed: f64,
}
impl Travel {
    pub fn new(position: Point, velocity: Point, target: Point) -> Self {
        let distance = (target[0] - position[0]).hypot(target[1] - position[1]);
        Self {
            position,
            velocity,
            target,
            // A bounded increase keeps longer trips responsive without giving
            // them the same duration as short trips.
            speed: 1200. * (1. + 0.3 * distance / (distance + 480.)),
        }
    }
    pub fn translate(&mut self, offset: Point) {
        for i in 0..2 { self.position[i] += offset[i]; self.target[i] += offset[i]; }
    }
    pub fn step(&mut self, dt: f64) {
        let delta = [
            self.target[0] - self.position[0],
            self.target[1] - self.position[1],
        ];
        let distance = delta[0].hypot(delta[1]);
        if distance < 0.01 && self.velocity[0].hypot(self.velocity[1]) < 1. {
            self.finish();
            return;
        }
        const ACCELERATION: f64 = 40000.;
        let step = ACCELERATION * dt;
        let braking = ((step * step + 2. * ACCELERATION * distance).sqrt() - step).max(0.);
        let desired = if distance > 0.000001 {
            delta.map(|v| v / distance * self.speed.min(braking))
        } else {
            [0., 0.]
        };
        let change = [desired[0] - self.velocity[0], desired[1] - self.velocity[1]];
        let magnitude = change[0].hypot(change[1]);
        let amount = if magnitude > step {
            step / magnitude
        } else {
            1.
        };
        for i in 0..2 {
            let velocity = self.velocity[i] + change[i] * amount;
            self.position[i] += (self.velocity[i] + velocity) * dt / 2.;
            self.velocity[i] = velocity;
        }
        let remaining = [
            self.target[0] - self.position[0],
            self.target[1] - self.position[1],
        ];
        if delta[0] * remaining[0] + delta[1] * remaining[1] <= 0.
            && remaining[0].hypot(remaining[1]) < 0.5
        {
            self.finish();
        }
    }
    pub fn finish(&mut self) {
        self.position = self.target;
        self.velocity = [0., 0.];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn longer_trips_take_longer_with_bounded_speed_increase() {
        let mut durations = Vec::new();
        let mut peaks = Vec::new();
        for distance in [32., 128., 512.] {
            let mut travel = Travel::new([0., 0.], [0., 0.], [distance, 0.]);
            let mut peak = 0_f64;
            let mut frames = 0;
            while travel.position != [distance, 0.] || travel.velocity != [0., 0.] {
                travel.step(1. / 240.);
                frames += 1;
                peak = peak.max(travel.velocity[0]);
                assert!(travel.position[0] >= 0. && travel.position[0] <= distance + 0.01);
                assert!(frames < 240);
            }
            durations.push(frames);
            peaks.push(peak);
        }
        assert!(
            durations[0] < durations[1] && durations[1] < durations[2],
            "{durations:?}"
        );
        assert!(peaks[2] / peaks[0] < 1.5, "{peaks:?}");
    }
    #[test]
    fn reversal_retains_position_and_velocity_then_brakes() {
        let mut travel = Travel::new([0., 0.], [0., 0.], [300., 0.]);
        for _ in 0..20 {
            travel.step(1. / 240.);
        }
        let position = travel.position;
        let velocity = travel.velocity;
        let mut reversed = Travel::new(position, velocity, [0., 0.]);
        assert_eq!(reversed.position, position);
        assert_eq!(reversed.velocity, velocity);
        reversed.step(1. / 240.);
        assert!(reversed.position[0] > position[0]);
        assert!(reversed.velocity[0] < velocity[0]);
        for _ in 0..240 {
            reversed.step(1. / 240.);
        }
        assert_eq!(reversed.position, [0., 0.]);
        assert_eq!(reversed.velocity, [0., 0.]);
    }
}
