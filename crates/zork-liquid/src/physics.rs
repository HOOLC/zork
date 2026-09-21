//! Stable, mass-weighted material samples transported by a compliant carrier.
use super::geometry::{contact_axis, fit_area, length, mix, Point, Pose, Prepared};
use std::collections::HashMap;

pub const FIXED_DT: f64 = 1. / 240.;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Material {
    pub budget: usize,
    pub flow: f64,
    pub damping: f64,
    pub recovery: f64,
    pub size_rate: f64,
    pub adhesion: f64,
    pub smoothing: f64,
    pub tension: f64,
    pub response: f64,
    pub position_damping: f64,
    pub size_damping: f64,
    /// Extra elastic travel in logical pixels, independent of component size.
    /// None retains the original unbounded reference material.
    pub rebound_limit: Option<f64>,
    pub layout_response: f64,
    pub motion_rounding: f64,
    pub surface_detail: f64,
    pub fusion_gain: f64,
}
impl Default for Material {
    fn default() -> Self {
        Self {
            budget: 12,
            flow: 0.08,
            damping: 1.,
            recovery: 1.5,
            size_rate: 1.,
            adhesion: 0.72,
            smoothing: crate::tokens::SMOOTHING,
            tension: 1.,
            response: 0.96,
            position_damping: 0.66,
            size_damping: 0.7,
            rebound_limit: Some(4.),
            layout_response: 1.,
            motion_rounding: 1.,
            surface_detail: 0.08,
            fusion_gain: 1.,
        }
    }
}
impl Material {
    /// Non-liquid motion for product UI: smooth corners stay, flow/fusion do not.
    pub fn ordinary() -> Self {
        Self {
            budget: 12,
            flow: 0.,
            damping: 1.,
            recovery: 1.5,
            size_rate: 1.,
            adhesion: 0.,
            smoothing: crate::tokens::SMOOTHING,
            tension: 1.,
            response: 1.15,
            position_damping: 0.78,
            size_damping: 0.82,
            rebound_limit: Some(2.),
            layout_response: 1.,
            motion_rounding: 0.,
            surface_detail: 0.,
            fusion_gain: 0.,
        }
    }

    /// Product UI materials do not morph between source and target poses.
    pub fn morphs(self) -> bool {
        self.fusion_gain > 0.
            || self.flow > 0.
            || self.surface_detail > 0.
            || self.adhesion > 0.
            || self.motion_rounding > 0.
    }

    pub fn valid(self) -> bool {
        (6..=64).contains(&self.budget)
            && [
                self.flow,
                self.damping,
                self.recovery,
                self.size_rate,
                self.adhesion,
                self.smoothing,
                self.tension,
                self.response,
                self.position_damping,
                self.size_damping,
                self.layout_response,
                self.motion_rounding,
                self.surface_detail,
                self.fusion_gain,
            ]
            .into_iter()
            .all(|x| x.is_finite() && x >= 0.)
            && self.damping > 0.
            && self.recovery > 0.
            && self.size_rate > 0.
            && self.tension > 0.
            && self.response > 0.
            && self.position_damping > 0.
            && self.size_damping > 0.
            && self
                .rebound_limit
                .is_none_or(|limit| limit.is_finite() && limit > 0.)
            && self.smoothing <= 1.
            && self.layout_response <= 1.
    }
    pub(crate) fn verified_sparse(self) -> bool {
        if !(0. ..=0.4).contains(&self.flow)
            || !(0.4..=2.).contains(&self.damping)
            || !(0. ..=1.2).contains(&self.adhesion)
            || !(0. ..=1.).contains(&self.smoothing)
        {
            return false;
        }
        // The interactive gallery's bounded controls are checked against dense
        // tracing. Other material changes still take the diagnostic fallback.
        let mut value = self;
        let defaults = Self::default();
        value.budget = 12;
        value.flow = defaults.flow;
        value.damping = defaults.damping;
        value.adhesion = defaults.adhesion;
        value.smoothing = defaults.smoothing;
        if value.rebound_limit.is_none() {
            value.rebound_limit = defaults.rebound_limit;
        }
        value == defaults
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub position: f64,
    pub velocity: f64,
    pub target: f64,
}
impl Spring {
    pub fn new(position: f64) -> Self {
        Self {
            position,
            velocity: 0.,
            target: position,
        }
    }
    pub fn step(&mut self, dt: f64, omega: f64, zeta: f64) {
        let d = self.position - self.target;
        let beta = zeta * omega;
        let v = self.velocity;
        if (zeta - 1.).abs() < 1e-6 {
            let b = v + omega * d;
            let e = (-omega * dt).exp();
            self.position = self.target + (d + b * dt) * e;
            self.velocity = (v - omega * b * dt) * e;
        } else if zeta < 1. {
            let q = omega * (1. - zeta * zeta).sqrt();
            let e = (-beta * dt).exp();
            let (s, c) = (q * dt).sin_cos();
            self.position = self.target + e * (d * c + (v + beta * d) * s / q);
            self.velocity = e * (v * c - (beta * v + omega * omega * d) * s / q);
        } else {
            let q = omega * (zeta * zeta - 1.).sqrt();
            let a = -beta + q;
            let b = -beta - q;
            let u = (v - b * d) / (a - b);
            let v = d - u;
            self.position = self.target + u * (a * dt).exp() + v * (b * dt).exp();
            self.velocity = a * u * (a * dt).exp() + b * v * (b * dt).exp();
        }
    }
    pub fn snap(&mut self) {
        self.position = self.target;
        self.velocity = 0.;
    }
    pub fn near(self, position: f64, velocity: f64) -> bool {
        (self.position - self.target).abs() < position && self.velocity.abs() < velocity
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Constraints {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub seed: u32,
    pub anchor: Point,
    pub capacity: f64,
    pub lock_area: bool,
    pub constraints: Constraints,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            seed: 7321,
            anchor: [0.5, 0.5],
            capacity: 0.,
            lock_area: false,
            constraints: Constraints::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Body {
    travel: Option<crate::travel::Travel>,
    pub channels: [Spring; 5],
    pub tether: [Spring; 2],
    pub locality: Spring,
    pub anchor: Point,
    pub compliant: bool,
    pub goal: Pose,
    pub omega: f64,
    axis: Point,
    speed: f64,
    drift: Point,
    start: [f64; 5],
    span: [f64; 5],
    round_mix: f64,
    area_lock: Option<f64>,
}
impl Body {
    fn new(p: Pose, m: Material, o: Options, compliant: bool) -> Self {
        let [u, v] = o.anchor;
        Self {
            travel: None,
            channels: [
                Spring::new(p.cx + (u - 0.5) * p.w),
                Spring::new(p.cy + (v - 0.5) * p.h),
                Spring::new(p.w),
                Spring::new(p.h),
                Spring::new(p.r),
            ],
            tether: [Spring::new(0.); 2],
            locality: Spring::new((1. - 2. * u).abs().max((1. - 2. * v).abs())),
            anchor: o.anchor,
            compliant,
            goal: p,
            omega: 19.,
            axis: [0., 1.],
            speed: 1.,
            drift: [0., 0.],
            start: [
                p.cx + (u - 0.5) * p.w,
                p.cy + (v - 0.5) * p.h,
                p.w,
                p.h,
                p.r,
            ],
            span: [0.; 5],
            round_mix: 0.,
            area_lock: o
                .lock_area
                .then(|| Prepared::new(p, m.smoothing, None).area()),
        }
    }
    pub fn pose(&self) -> Pose {
        let c = &self.channels;
        let [u, v] = self.anchor;
        Pose {
            cx: c[0].position + self.tether[0].position + (0.5 - u) * c[2].position,
            cy: c[1].position + self.tether[1].position + (0.5 - v) * c[3].position,
            w: c[2].position,
            h: c[3].position,
            r: c[4].position,
            vx: c[0].velocity + self.tether[0].velocity + (0.5 - u) * c[2].velocity,
            vy: c[1].velocity + self.tether[1].velocity + (0.5 - v) * c[3].velocity,
            vw: c[2].velocity,
            vh: c[3].velocity,
        }
    }
    pub fn target(&mut self, mut p: Pose, speed: f64, m: Material) {
        self.travel = None;
        if let Some(area) = self.area_lock {
            p = fit_area(p, area, m.smoothing);
        }
        let before = self.pose();
        let delta = [p.cx - before.cx, p.cy - before.cy];
        let distance = length(delta);
        if distance > 0.01 {
            self.axis = [delta[0] / distance, delta[1] / distance];
        }
        let travel = distance + 0.5 * length([p.w - before.w, p.h - before.h]);
        self.omega = mix(
            19.,
            (4500. / (travel + 64.)).clamp(17., 38.),
            m.layout_response,
        );
        self.goal = p;
        self.speed = speed;
        for (i, (c, target)) in self
            .channels
            .iter_mut()
            .zip([
                p.cx + (self.anchor[0] - 0.5) * p.w,
                p.cy + (self.anchor[1] - 0.5) * p.h,
                p.w,
                p.h,
                p.r,
            ])
            .enumerate()
        {
            self.start[i] = c.position;
            self.span[i] = (target - c.position).abs() + c.velocity.abs() / self.omega.max(1.);
            c.target = target;
        }
        self.drift = [p.vx, p.vy];
    }
    fn translate(&mut self, offset: Point) {
        for i in 0..2 {
            self.channels[i].position += offset[i];
            self.channels[i].target += offset[i];
            self.start[i] += offset[i];
        }
        self.goal.cx += offset[0];
        self.goal.cy += offset[1];
        if let Some(travel) = &mut self.travel {
            travel.translate(offset);
        }
    }
    fn set_anchor(&mut self, anchor: Point, m: Material) {
        let p = self.pose();
        self.anchor = anchor;
        for i in 0..2 {
            self.channels[i].position =
                [p.cx, p.cy][i] + (anchor[i] - 0.5) * [p.w, p.h][i] - self.tether[i].position;
            self.channels[i].velocity =
                [p.vx, p.vy][i] + (anchor[i] - 0.5) * [p.vw, p.vh][i] - self.tether[i].velocity;
        }
        self.target(self.goal, self.speed, m);
    }
    pub fn prepared(&self, m: Material) -> Prepared {
        let mut p = self.pose();
        let r = p.r.min(p.short() / 2.);
        // Speed may soften a corner, but must not turn a large panel into a
        // capsule. Keep additional roundness in the same pixel budget as rebound.
        let round = m
            .rebound_limit
            .map_or(p.short() / 2., |limit| (r + limit).min(p.short() / 2.));
        let corners = if self.compliant && self.locality.position > 0. {
            Some([[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]].map(|[x, y]| {
                let speed = length([p.vx + x * 0.5 * p.vw, p.vy + y * 0.5 * p.vh]);
                mix(
                    r,
                    round,
                    mix(
                        self.round_mix,
                        m.motion_rounding * (1. - (-(speed / 650.).powi(2)).exp()),
                        self.locality.position,
                    ),
                )
            }))
        } else {
            None
        };
        p.r = mix(r, round, self.round_mix);
        Prepared::new(p, m.smoothing, corners)
    }
    fn step(&mut self, dt: f64, m: Material) {
        let before = self.pose();
        let omega = self.omega * self.speed * m.tension.sqrt() * m.response;
        let rates = [
            mix(1., 1.25, m.layout_response),
            mix(1., 1.25, m.layout_response),
            mix(0.72, 0.84 + 0.36 * self.axis[0].abs(), m.layout_response) * m.size_rate,
            mix(0.72, 0.84 + 0.36 * self.axis[1].abs(), m.layout_response) * m.size_rate,
            m.size_rate,
        ];
        for i in 0..2 {
            self.channels[i].target += self.drift[i] * dt;
        }
        for (i, c) in self.channels.iter_mut().enumerate() {
            if i < 2 && self.travel.is_some() {
                continue;
            }
            let frequency = omega * rates[i];
            let base = if i < 2 {
                m.position_damping
            } else if i < 4 {
                m.size_damping
            } else {
                0.84
            };
            let damping = if let Some(limit) = m.rebound_limit {
                let limit = if i < 2 { limit * 0.5 } else { limit };
                let ratio = (limit / self.span[i].max(limit)).ln();
                // The analytic spring's step overshoot is exp(-pi*zeta /
                // sqrt(1-zeta^2)). Solve for a fixed pixel excursion.
                let mut damping =
                    base.max(-ratio / (std::f64::consts::PI.powi(2) + ratio * ratio).sqrt());
                if (c.position - c.target) * c.velocity > 0. {
                    // Brake outward momentum on a reversal without resetting
                    // the painted position or velocity. Release the extra
                    // damping as soon as motion points toward the target.
                    let low = self.start[i].min(c.target);
                    let high = self.start[i].max(c.target);
                    let excess = (low - c.position).max(c.position - high).max(0.);
                    let remaining = (limit - excess).max(limit * 0.1);
                    damping = damping.max(c.velocity.abs() / (2. * frequency * remaining));
                }
                damping
            } else {
                base
            };
            c.step(dt, frequency, damping);
        }
        for i in 2..4 {
            if self.channels[i].position < 2. {
                self.channels[i].position = 2.;
                self.channels[i].velocity = self.channels[i].velocity.max(0.);
            }
        }
        if self.compliant {
            self.locality.target = (1. - 2. * self.anchor[0])
                .abs()
                .max((1. - 2. * self.anchor[1]).abs());
            self.locality.step(dt, omega * 1.1, 1.);
            let extent = self.channels[2].position.min(self.channels[3].position) * 0.2;
            let limit = m
                .rebound_limit
                .map_or(extent, |limit| extent.min(limit * 0.5));
            for i in 0..2 {
                self.tether[i].target = limit
                    * ((1. - 2. * self.anchor[i]) * self.channels[i + 2].velocity * 0.4
                        / omega.max(1.)
                        / limit)
                        .tanh();
                self.tether[i].step(dt, omega * 1.1, 0.7);
            }
        }
        if let Some(travel) = &mut self.travel {
            travel.step(dt);
            for i in 0..2 {
                self.channels[i].position = travel.position[i]
                    - self.tether[i].position
                    - (0.5 - self.anchor[i]) * self.channels[i + 2].position;
                self.channels[i].velocity = travel.velocity[i]
                    - self.tether[i].velocity
                    - (0.5 - self.anchor[i]) * self.channels[i + 2].velocity;
            }
        }
        let p = self.pose();
        let speed = (p.vx * p.vx + p.vy * p.vy + 0.1225 * (p.vw * p.vw + p.vh * p.vh)).sqrt();
        self.round_mix = m.motion_rounding * (1. - (-(speed / 650.).powi(2)).exp());
        if let Some(area) = self.area_lock {
            let scale = (area / self.prepared(m).area()).sqrt();
            for i in 2..5 {
                self.channels[i].position *= scale;
                self.channels[i].velocity =
                    (self.channels[i].position - [before.w, before.h, before.r][i - 2]) / dt;
            }
        }
    }
    fn moving(&self) -> bool {
        (self.compliant && !self.locality.near(1e-7, 2e-6))
            || self.tether.iter().any(|c| !c.near(2e-5, 2e-4))
            || self.channels.iter().any(|c| !c.near(2e-5, 2e-4))
            || length(self.drift) > 0.001
    }
    fn remaining_motion(&self, m: Material) -> f64 {
        if self.drift != [0., 0.] {
            return f64::INFINITY;
        }
        // Bound both displacement and momentum in the same spatial units.
        // A spring crossing its target at speed is not at rest.
        let frequency =
            self.omega * self.speed * m.tension.sqrt() * m.response * (0.72 * m.size_rate).min(1.);
        let goal = self.goal;
        let targets = [
            goal.cx + (self.anchor[0] - 0.5) * goal.w,
            goal.cy + (self.anchor[1] - 0.5) * goal.h,
            goal.w,
            goal.h,
            goal.r,
        ];
        let remaining = std::array::from_fn::<_, 5, _>(|i| {
            (self.channels[i].position - targets[i]).abs()
                + self.channels[i].velocity.abs() / frequency
        });
        let edge = (0..2)
            .map(|i| {
                remaining[i]
                    + self.anchor[i].max(1. - self.anchor[i]) * remaining[i + 2]
                    + self.tether[i].position.abs()
                    + self.tether[i].velocity.abs() / frequency
            })
            .fold(remaining[4], f64::max);
        let rounded = m.rebound_limit.unwrap_or(goal.short() * 0.5) * self.round_mix.abs();
        edge + rounded
    }
    fn finish(&mut self) {
        if let Some(travel) = &mut self.travel {
            travel.finish();
        }
        self.locality.target = (1. - 2. * self.anchor[0])
            .abs()
            .max((1. - 2. * self.anchor[1]).abs());
        self.locality.snap();
        for c in &mut self.tether {
            c.target = 0.;
            c.snap();
        }
        for c in &mut self.channels {
            c.snap();
        }
        self.drift = [0., 0.];
        self.round_mix = 0.;
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Particle {
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub qx: f64,
    pub qy: f64,
    pub vx: f64,
    pub vy: f64,
    pub mass: f64,
    #[serde(skip)]
    angle: f64,
    #[serde(skip)]
    material_angle: f64,
    #[serde(skip)]
    radial_fraction: f64,
    #[serde(skip)]
    drive: f64,
    #[serde(skip)]
    previous: Point,
}
fn halton(mut i: u32, base: u32) -> f64 {
    let (mut x, mut f) = (0., 1.);
    while i > 0 {
        f /= base as f64;
        x += f * (i % base) as f64;
        i /= base;
    }
    x
}

#[derive(Clone, Debug)]
pub(crate) struct Group {
    pub body: Body,
    pub prepared: Prepared,
    pub particles: Vec<Particle>,
    pub area: f64,
    pub sigma: f64,
    pub coverage: f64,
    pub effective_count: f64,
    pub capacity: f64,
    pub affine: bool,
    pub axial: [Spring; 2],
    pub constraints: Constraints,
    pub max_error: f64,
    pub max_speed: f64,
    volume: Option<f64>,
    seed: u32,
    next_id: u32,
    sequence: u32,
    bonds: HashMap<(u32, u32), f64>,
    pairs: Vec<(usize, usize)>,
    needs_step: bool,
}
impl Group {
    fn new(p: Pose, m: Material, o: Options, affine: bool, volume: Option<f64>) -> Self {
        let mut body = Body::new(p, m, o, affine);
        body.finish();
        let prepared = body.prepared(m);
        let mut g = Self {
            body,
            prepared,
            particles: Vec::new(),
            area: 0.,
            sigma: 0.,
            coverage: 0.,
            effective_count: 0.,
            capacity: o.capacity.max(p.w * p.h).max(4200.),
            affine,
            axial: [
                Spring::new(
                    if o.constraints.top
                        && o.constraints.bottom
                        && !(o.constraints.left && o.constraints.right)
                    {
                        1.
                    } else {
                        0.
                    },
                ),
                Spring::new(
                    if o.constraints.left
                        && o.constraints.right
                        && !(o.constraints.top && o.constraints.bottom)
                    {
                        1.
                    } else {
                        0.
                    },
                ),
            ],
            constraints: o.constraints,
            max_error: 0.,
            max_speed: 0.,
            volume,
            seed: o.seed,
            next_id: 1,
            sequence: 0,
            bonds: HashMap::new(),
            pairs: Vec::new(),
            needs_step: false,
        };
        g.refresh(m);
        let mass = g.area / m.budget as f64;
        for _ in 0..m.budget {
            g.make(mass);
        }
        g.refresh(m);
        g
    }
    pub fn mass(&self) -> f64 {
        self.particles.iter().map(|p| p.mass).sum()
    }
    fn refresh(&mut self, m: Material) {
        self.prepared = self.body.prepared(m);
        let geometric = self.prepared.area();
        self.area = self
            .volume
            .or(self.body.area_lock)
            .unwrap_or_else(|| Prepared::new(self.body.pose(), m.smoothing, None).area());
        self.coverage = self.area / geometric;
        self.effective_count = (m.budget as f64 * geometric / self.capacity).max(6.);
        self.sigma = ((geometric / self.effective_count).sqrt() * 0.85).max(0.6);
    }
    fn point(&self, a: f64, r: f64) -> Point {
        let radius = self.prepared.radial(a) * r;
        [a.cos() * radius, a.sin() * radius]
    }
    fn material_point(&self, p: &Particle) -> Point {
        let pose = self.prepared.pose;
        let a = if self.affine {
            (p.material_angle.sin() * pose.h).atan2(p.material_angle.cos() * pose.w)
        } else {
            p.angle
        };
        self.point(a, p.radial_fraction)
    }
    fn make(&mut self, mass: f64) {
        self.sequence += 1;
        let index = self.sequence + self.seed % 31;
        let a = halton(index, 2) * std::f64::consts::TAU;
        let r = halton(index, 3).sqrt() * 0.93;
        let q = self.point(a, r);
        let pose = self.prepared.pose;
        self.particles.push(Particle {
            id: self.next_id,
            angle: a,
            material_angle: (a.sin() / pose.h).atan2(a.cos() / pose.w),
            radial_fraction: r,
            mass,
            x: q[0],
            y: q[1],
            qx: q[0],
            qy: q[1],
            vx: 0.,
            vy: 0.,
            drive: 0.,
            previous: q,
        });
        self.next_id += 1;
    }
    fn resample(&mut self, m: Material) {
        self.needs_step = true;
        while self.particles.len() > m.budget {
            let (mut best, mut ai, mut bi) = (f64::INFINITY, 0, 1);
            for i in 0..self.particles.len() {
                for j in i + 1..self.particles.len() {
                    let a = &self.particles[i];
                    let b = &self.particles[j];
                    let d = (a.x - b.x).powi(2) + (a.y - b.y).powi(2);
                    if d < best {
                        best = d;
                        ai = i;
                        bi = j;
                    }
                }
            }
            let b = self.particles.remove(bi);
            let a = &mut self.particles[ai];
            let total = a.mass + b.mass;
            let t = b.mass / total;
            a.x = mix(a.x, b.x, t);
            a.y = mix(a.y, b.y, t);
            a.vx = mix(a.vx, b.vx, t);
            a.vy = mix(a.vy, b.vy, t);
            a.qx = mix(a.qx, b.qx, t);
            a.qy = mix(a.qy, b.qy, t);
            a.mass = total;
            a.angle = a.qy.atan2(a.qx);
            a.material_angle = (a.qy / self.prepared.pose.h).atan2(a.qx / self.prepared.pose.w);
            a.radial_fraction = length([a.qx, a.qy]) / self.prepared.radial(a.angle).max(0.001);
        }
        while self.particles.len() < m.budget {
            let i = self
                .particles
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.mass.total_cmp(&b.mass))
                .unwrap()
                .0;
            self.particles[i].mass *= 0.5;
            let mut q = self.particles[i].clone();
            q.id = self.next_id;
            self.next_id += 1;
            self.particles.push(q);
        }
        self.bonds.clear();
        self.refresh(m);
    }
    fn step(&mut self, dt: f64, m: Material, before: Option<Pose>) {
        self.needs_step = false;
        let before = before.unwrap_or_else(|| {
            let p = self.body.pose();
            self.body.step(dt, m);
            p
        });
        self.refresh(m);
        let pose = self.body.pose();
        let short = pose.short();
        let acceleration = [(pose.vx - before.vx) / dt, (pose.vy - before.vy) / dt];
        let velocity = length([pose.vx, pose.vy]);
        let axis = if velocity > 1. {
            [pose.vx / velocity, pose.vy / velocity]
        } else {
            [1., 0.]
        };
        let sx = pose.vw.abs() / pose.w.max(2.);
        let sy = pose.vh.abs() / pose.h.max(2.);
        let walls = self.constraints;
        self.axial[0].target = if walls.top && walls.bottom && !(walls.left && walls.right) {
            1.
        } else {
            (sx - 3. * sy).max(0.) / (sx + 0.08)
        };
        self.axial[1].target = if walls.left && walls.right && !(walls.top && walls.bottom) {
            1.
        } else {
            (sy - 3. * sx).max(0.) / (sy + 0.08)
        };
        for s in &mut self.axial {
            s.step(dt, 80., 1.);
        }
        let c = &self.body.channels;
        let motion = [
            (c[0].target - c[0].position) / (short * 0.6),
            (c[1].target - c[1].position) / (short * 0.6),
            velocity / (short * 4.),
            pose.vw / (short * 3.),
            pose.vh / (short * 3.),
        ]
        .into_iter()
        .map(|x| x * x)
        .sum::<f64>()
        .sqrt();
        let t = ((1. - motion) / 0.85).clamp(0., 1.);
        let capture = t * t * (3. - 2. * t);
        let k = 105. + 1000. * capture * m.recovery;
        let drag = (6. + 61. * capture * m.recovery.sqrt()) * m.damping;
        let mass = self.mass();
        let mut mean = 0.;
        for i in 0..self.particles.len() {
            let q = self.material_point(&self.particles[i]);
            let p = &mut self.particles[i];
            p.x += (q[0] - p.qx) * 0.55;
            p.y += (q[1] - p.qy) * 0.55;
            p.qx = q[0];
            p.qy = q[1];
            let perpendicular = -axis[1] * q[0] + axis[0] * q[1];
            p.drive = 1.3 * (-(perpendicular / (short * 0.25)).powi(2)).exp() - 0.5;
            mean += p.drive * p.mass;
        }
        mean /= mass;
        for p in &mut self.particles {
            p.vx +=
                dt * (k * (p.qx - p.x) - drag * p.vx + acceleration[0] * (p.drive - mean) * m.flow);
            p.vy +=
                dt * (k * (p.qy - p.y) - drag * p.vy + acceleration[1] * (p.drive - mean) * m.flow);
            p.previous = [p.x, p.y];
        }
        let h = (self.area / self.effective_count).sqrt() * 2.5;
        let mean_mass = mass / self.effective_count;
        let mut pairs = std::mem::take(&mut self.pairs);
        pairs.clear();
        for i in 0..self.particles.len() {
            for j in i + 1..self.particles.len() {
                let (a, b) = two_mut(&mut self.particles, i, j);
                let dx = b.x - a.x;
                let dy = b.y - a.y;
                let r = length([dx, dy]).max(0.001);
                let reference = length([b.qx - a.qx, b.qy - a.qy]);
                if r < h || reference < h {
                    pairs.push((i, j));
                    if r < h {
                        let nx = dx / r;
                        let ny = dy / r;
                        let u = (a.vx - b.vx) * nx + (a.vy - b.vy) * ny;
                        if u > 0. {
                            let dv = (u * 0.9)
                                .min(dt * (1. - r / h) * (10. * u + 0.012 * u * u) * m.damping);
                            let sum = a.mass + b.mass;
                            a.vx -= dv * nx * b.mass / sum;
                            a.vy -= dv * ny * b.mass / sum;
                            b.vx += dv * nx * a.mass / sum;
                            b.vy += dv * ny * a.mass / sum;
                        }
                    }
                }
            }
        }
        for p in &mut self.particles {
            p.x += p.vx * dt;
            p.y += p.vy * dt;
        }
        for &(i, j) in &pairs {
            let (a, b) = two_mut(&mut self.particles, i, j);
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let r = length([dx, dy]).max(0.001);
            let reference = length([b.qx - a.qx, b.qy - a.qy]);
            let rest = self.bonds.entry((a.id, b.id)).or_insert(r);
            *rest += (reference - *rest) * (1. - (-7. * dt).exp());
            let d = dt * dt * 150. * (1. - *rest / h).max(0.) * (*rest - r);
            let sum = a.mass + b.mass;
            a.x -= dx / r * d * b.mass / sum;
            a.y -= dy / r * d * b.mass / sum;
            b.x += dx / r * d * a.mass / sum;
            b.y += dy / r * d * a.mass / sum;
        }
        let (mut rho, mut near, mut rr, mut rn) = ([0.; 64], [0.; 64], [0.; 64], [0.; 64]);
        for &(i, j) in &pairs {
            let a = &self.particles[i];
            let b = &self.particles[j];
            let q = (1. - length([b.x - a.x, b.y - a.y]) / h).max(0.);
            let r = (1. - length([b.qx - a.qx, b.qy - a.qy]) / h).max(0.);
            let ma = a.mass / mean_mass;
            let mb = b.mass / mean_mass;
            rho[i] += mb * q * q;
            rho[j] += ma * q * q;
            near[i] += mb * q * q * q;
            near[j] += ma * q * q * q;
            rr[i] += mb * r * r;
            rr[j] += ma * r * r;
            rn[i] += mb * r * r * r;
            rn[j] += ma * r * r * r;
        }
        for &(i, j) in &pairs {
            let (a, b) = two_mut(&mut self.particles, i, j);
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let r = length([dx, dy]).max(0.001);
            let q = (1. - r / h).max(0.);
            let pressure = 1500. * ((rho[i] - rr[i]) + (rho[j] - rr[j])) * 0.5;
            let near_pressure = 1500. * 0.65 * ((near[i] - rn[i]) + (near[j] - rn[j])) * 0.5;
            let d = (dt * dt * (pressure * q + near_pressure * q * q)).clamp(-h * 0.04, h * 0.04);
            let sum = a.mass + b.mass;
            a.x -= dx / r * d * b.mass / sum;
            a.y -= dy / r * d * b.mass / sum;
            b.x += dx / r * d * a.mass / sum;
            b.y += dy / r * d * a.mass / sum;
        }
        let ex = self
            .particles
            .iter()
            .map(|p| (p.x - p.qx) * p.mass)
            .sum::<f64>()
            / mass;
        let ey = self
            .particles
            .iter()
            .map(|p| (p.y - p.qy) * p.mass)
            .sum::<f64>()
            / mass;
        self.max_error = 0.;
        self.max_speed = 0.;
        for p in &mut self.particles {
            p.x -= ex;
            p.y -= ey;
            let limit = (short * 0.7).max(1.);
            let d = length([p.x - p.qx, p.y - p.qy]);
            if d > limit {
                p.x = p.qx + (p.x - p.qx) * limit / d;
                p.y = p.qy + (p.y - p.qy) * limit / d;
            }
            p.vx = (p.x - p.previous[0]) / dt;
            p.vy = (p.y - p.previous[1]) / dt;
            self.max_error = self.max_error.max(length([p.x - p.qx, p.y - p.qy]));
            self.max_speed = self.max_speed.max(length([p.vx, p.vy]));
        }
        self.pairs = pairs;
        self.refresh(m);
    }
    fn moving(&self) -> bool {
        self.needs_step
            || self.body.moving()
            || self.max_error > 0.00008
            || self.max_speed > 0.001
            || (self.affine && self.axial.iter().any(|s| !s.near(0.0001, 0.001)))
    }
    fn finish(&mut self, m: Material) {
        self.needs_step = false;
        self.body.finish();
        self.refresh(m);
        for i in 0..self.particles.len() {
            let q = self.material_point(&self.particles[i]);
            let p = &mut self.particles[i];
            p.x = q[0];
            p.qx = q[0];
            p.y = q[1];
            p.qy = q[1];
            p.vx = 0.;
            p.vy = 0.;
        }
        self.max_error = 0.;
        self.max_speed = 0.;
        let c = self.constraints;
        self.axial[0].target = if c.top && c.bottom && !(c.left && c.right) {
            1.
        } else {
            0.
        };
        self.axial[1].target = if c.left && c.right && !(c.top && c.bottom) {
            1.
        } else {
            0.
        };
        for s in &mut self.axial {
            s.snap();
        }
    }
}
fn two_mut<T>(values: &mut [T], i: usize, j: usize) -> (&mut T, &mut T) {
    let (left, right) = values.split_at_mut(j);
    (&mut left[i], &mut right[0])
}

#[derive(Clone, Debug)]
enum Mode {
    Single,
    Compound,
    Pair {
        source: Pose,
        target: Pose,
        axis: Point,
        open: bool,
    },
    Split {
        closed: Pose,
        targets: Vec<Pose>,
        open: bool,
    },
}

/// Pure Rust simulation. No window, event loop, network or persistence owner.
#[derive(Clone, Debug)]
pub struct Simulation {
    pub(crate) material: Material,
    pub(crate) groups: Vec<Group>,
    pub(crate) fusion: f64,
    mode: Mode,
    pub revision: u64,
    dirty: bool,
    accumulator: f64,
}
impl Simulation {
    pub fn new(pose: Pose, material: Material, options: Options) -> Self {
        assert!(
            pose.is_valid() && material.valid(),
            "invalid liquid geometry or material"
        );
        Self {
            material,
            fusion: 0.,
            groups: vec![Group::new(pose, material, options, true, None)],
            mode: Mode::Single,
            revision: 0,
            dirty: true,
            accumulator: 0.,
        }
    }
    pub fn pair(source: Pose, target: Pose, material: Material, options: Options) -> Self {
        assert!(source.is_valid() && target.is_valid() && material.valid());
        let a = Group::new(
            source,
            material,
            Options {
                capacity: source.w * source.h,
                ..options
            },
            false,
            None,
        );
        let b = Group::new(source, material, options, false, None);
        Self {
            material,
            fusion: 0.,
            groups: vec![a, b],
            mode: Mode::Pair {
                source,
                target,
                axis: contact_axis(source, target),
                open: false,
            },
            revision: 0,
            dirty: true,
            accumulator: 0.,
        }
    }
    pub fn split(
        closed: Pose,
        targets: &[(Pose, f64)],
        material: Material,
        options: Options,
    ) -> Self {
        assert!(!targets.is_empty() && targets.len() <= 8 && closed.is_valid() && material.valid());
        assert!(
            targets
                .iter()
                .all(|(p, f)| p.is_valid() && f.is_finite() && *f > 0.)
                && (targets.iter().map(|(_, f)| f).sum::<f64>() - 1.).abs() < 1e-6
        );
        let amount = Prepared::new(closed, material.smoothing, None).area();
        let groups = targets
            .iter()
            .enumerate()
            .map(|(i, (_, f))| {
                Group::new(
                    closed,
                    material,
                    Options {
                        seed: options.seed.wrapping_add(i as u32 * 97),
                        capacity: closed.w * closed.h,
                        ..options
                    },
                    false,
                    Some(amount * f),
                )
            })
            .collect();
        Self {
            material,
            fusion: 0.,
            groups,
            mode: Mode::Split {
                closed,
                targets: targets
                    .iter()
                    .map(|(p, f)| fit_area(*p, amount * f, material.smoothing))
                    .collect(),
                open: false,
            },
            revision: 0,
            dirty: true,
            accumulator: 0.,
        }
    }
    /// Multiple persistent material parcels sharing a fused contour. Each target
    /// keeps its own identity and velocity while the compound layout changes.
    pub fn compound(poses: &[Pose], fusion: f64, material: Material, options: Options) -> Self {
        assert!(!poses.is_empty() && poses.iter().all(|p| p.is_valid()));
        assert!(fusion.is_finite() && fusion >= 0.);
        let mut result = Self::new(poses[0], material, options);
        result.groups = poses
            .iter()
            .enumerate()
            .map(|(i, pose)| {
                Group::new(
                    *pose,
                    material,
                    Options {
                        seed: options.seed.wrapping_add(i as u32 * 97),
                        capacity: pose.w * pose.h,
                        ..options
                    },
                    true,
                    None,
                )
            })
            .collect();
        result.mode = Mode::Compound;
        result.fusion = fusion;
        result
    }
    pub(crate) fn is_compound(&self) -> bool {
        matches!(self.mode, Mode::Compound)
    }
    /// Transport an existing scene with its host layout. This changes the
    /// coordinate origin, preserving deformation, velocity and remaining motion.
    pub fn translate(&mut self, offset: Point) {
        assert!(offset.iter().all(|value| value.is_finite()));
        if offset == [0., 0.] {
            return;
        }
        for group in &mut self.groups {
            group.body.translate(offset);
            group.refresh(self.material);
        }
        let shift = |pose: &mut Pose| {
            pose.cx += offset[0];
            pose.cy += offset[1];
        };
        match &mut self.mode {
            Mode::Pair { source, target, .. } => {
                shift(source);
                shift(target);
            }
            Mode::Split {
                closed, targets, ..
            } => {
                shift(closed);
                for target in targets {
                    shift(target);
                }
            }
            Mode::Single | Mode::Compound => {}
        }
        self.revision += 1;
    }
    pub fn set_compound_targets(&mut self, poses: &[Pose]) {
        assert_eq!(poses.len(), self.groups.len());
        for (group, pose) in self.groups.iter_mut().zip(poses) {
            assert!(pose.is_valid());
            if group.body.goal != *pose {
                group.capacity = group.capacity.max(pose.w * pose.h);
                group.body.target(*pose, 1., self.material);
                self.dirty = true;
            }
        }
    }
    pub fn group_pose(&self, index: usize) -> Pose {
        self.groups[index].body.pose()
    }
    pub fn set_compound_group_target(&mut self, index: usize, pose: Pose) {
        assert!(self.is_compound() && pose.is_valid());
        let group = &mut self.groups[index];
        if group.body.goal != pose {
            group.capacity = group.capacity.max(pose.w * pose.h);
            group.body.target(pose, 1., self.material);
            self.dirty = true;
        }
    }
    /// Read-only rendering projection of one parcel, retaining its material
    /// samples and current velocities. Advancing the parent remains the host's job.
    pub fn group_snapshot(&self, index: usize) -> Self {
        Self {
            material: self.material,
            groups: vec![self.groups[index].clone()],
            fusion: 0.,
            mode: if self.is_compound() {
                Mode::Compound
            } else {
                Mode::Single
            },
            revision: self.revision,
            dirty: false,
            accumulator: 0.,
        }
    }
    /// Spawn/retire a transient parcel without rebuilding the surviving bodies.
    pub fn append_compound_group(&mut self, pose: Pose) -> usize {
        let index = self.groups.len();
        self.insert_compound_group(index, pose);
        index
    }
    /// Insert a persistent parcel before transient effects without resetting
    /// the positions or velocities of any surviving group.
    pub fn insert_compound_group(&mut self, index: usize, pose: Pose) {
        assert!(self.is_compound() && index > 0 && index <= self.groups.len() && pose.is_valid());
        self.groups.insert(
            index,
            Group::new(
                pose,
                self.material,
                Options {
                    seed: self.groups[0].seed.wrapping_add(index as u32 * 97),
                    capacity: pose.w * pose.h,
                    ..Options::default()
                },
                true,
                None,
            ),
        );
        self.dirty = true;
        self.revision += 1;
    }
    pub fn remove_compound_group(&mut self, index: usize) {
        assert!(self.is_compound() && index > 0 && index < self.groups.len());
        self.groups.remove(index);
        self.dirty = true;
        self.revision += 1;
    }
    fn body_index(&self) -> usize {
        if matches!(self.mode, Mode::Pair { .. }) {
            1
        } else {
            0
        }
    }
    pub fn pose(&self) -> Pose {
        self.groups[self.body_index()].body.pose()
    }
    pub fn source_pose(&self) -> Pose {
        self.groups[0].body.pose()
    }
    pub fn source_partition(&self) -> crate::ownership::MaterialPartition {
        let Mode::Pair { axis, open, .. } = self.mode else {
            panic!("source_partition requires a pair");
        };
        crate::ownership::MaterialPartition::along(self.source_pose(), self.pose(), open, axis)
    }
    pub fn target_pose(&self) -> Pose {
        self.groups[self.body_index()].body.goal
    }
    pub fn omega(&self) -> f64 {
        self.groups[self.body_index()].body.omega * self.material.tension
    }
    pub fn material(&self) -> Material {
        self.material
    }
    pub fn particles(&self) -> impl Iterator<Item = &Particle> {
        self.groups.iter().flat_map(|g| g.particles.iter())
    }
    pub fn mass(&self) -> f64 {
        self.groups.iter().map(Group::mass).sum()
    }
    pub fn moving(&self) -> bool {
        self.dirty || self.groups.iter().any(Group::moving)
    }
    fn presentation_settled(&self) -> bool {
        // Numerical convergence is useful for fixed-step reference traces;
        // UI playback ends when its remaining contour motion is subpixel.
        const RESIDUAL: f64 = 0.05;
        let stiffness = 105. + 1000. * self.material.recovery;
        let drag = (6. + 61. * self.material.recovery.sqrt()) * self.material.damping;
        let particle_frequency = stiffness.sqrt().min(stiffness / drag);
        !self.dirty
            && self.groups.iter().all(|g| {
                !g.needs_step
                    && g.body.remaining_motion(self.material)
                        + self.material.surface_detail
                            * (g.max_error + g.max_speed / particle_frequency)
                        <= RESIDUAL
            })
    }
    pub fn set_target(&mut self, p: Pose) {
        self.set_target_with_speed(p, 1.);
    }
    pub fn set_target_with_speed(&mut self, p: Pose, speed: f64) {
        assert!(p.is_valid() && speed.is_finite() && speed > 0.);
        let i = self.body_index();
        self.groups[i].capacity = self.groups[i].capacity.max(p.w * p.h);
        self.groups[i].body.target(p, speed, self.material);
        self.dirty = true;
    }
    /// Retarget the centre at a common travel speed while keeping the current
    /// velocity and the existing size/rounding/particle simulation.
    pub fn set_travel_target(&mut self, target: Pose) {
        let before = self.pose();
        self.set_target(target);
        let i = self.body_index();
        self.groups[i].body.travel = Some(crate::travel::Travel::new(
            [before.cx, before.cy],
            [before.vx, before.vy],
            [target.cx, target.cy],
        ));
    }
    pub fn layout_pair(&mut self, source: Pose, target: Pose) {
        assert!(source.is_valid() && target.is_valid());
        let (open, previous) = match self.mode {
            Mode::Pair { open, source, .. } => (open, source),
            _ => panic!("layout_pair requires a pair"),
        };
        // Page layout transports the source. It is not a new physical target;
        // retain deformation/velocity relative to its actual control slot.
        let delta = [source.cx - previous.cx, source.cy - previous.cy];
        self.groups[0].body.translate(delta);
        self.mode = Mode::Pair {
            source,
            target,
            axis: contact_axis(source, target),
            open,
        };
        self.groups[0].body.target(source, 1., self.material);
        self.groups[0].refresh(self.material);
        if delta != [0., 0.] {
            self.revision += 1;
        }
        self.set_target(if open { target } else { source });
    }
    pub fn set_open(&mut self, value: bool) {
        match &mut self.mode {
            Mode::Pair {
                source,
                target,
                open,
                ..
            } => {
                *open = value;
                let p = if value { *target } else { *source };
                self.set_target(p);
            }
            Mode::Split {
                closed,
                targets,
                open,
            } => {
                *open = value;
                for (g, target) in self.groups.iter_mut().zip(targets) {
                    g.body
                        .target(if value { *target } else { *closed }, 1., self.material);
                }
                self.dirty = true;
            }
            Mode::Single | Mode::Compound => panic!("set_open requires a pair or split"),
        }
    }
    pub fn set_anchor(&mut self, anchor: Point, constraints: Constraints) {
        assert!(anchor
            .into_iter()
            .all(|x| x.is_finite() && (0. ..=1.).contains(&x)));
        let i = self.body_index();
        self.groups[i].body.set_anchor(anchor, self.material);
        self.groups[i].constraints = constraints;
        self.groups[i].needs_step = true;
        self.dirty = true;
    }
    pub fn configure(&mut self, material: Material) {
        assert!(material.valid());
        self.material = material;
        for g in &mut self.groups {
            g.resample(material);
        }
        self.dirty = true;
        self.revision += 1;
    }
    pub fn set_seed(&mut self, seed: u32) {
        for g in &mut self.groups {
            g.seed = seed;
        }
    }
    pub fn stop(&mut self) {
        let p = self.pose();
        self.set_target(Pose {
            cx: p.cx + p.vx / 32.,
            cy: p.cy + p.vy / 32.,
            vx: 0.,
            vy: 0.,
            ..p
        });
    }
    fn drive_source(&mut self, strength: f64) {
        let Mode::Pair { axis: e, .. } = self.mode else {
            return;
        };
        let b = self.groups[1].body.pose();
        let body = &mut self.groups[0].body;
        let a = body.pose();
        let rest = body.goal;
        let radius = (rest.short() / 2.).max(1.);
        let half_a = (e[0].abs() * a.w + e[1].abs() * a.h) / 2.;
        let half_b = (e[0].abs() * b.w + e[1].abs() * b.h) / 2.;
        let gap = (b.cx - a.cx) * e[0] + (b.cy - a.cy) * e[1] - half_a - half_b;
        let influence = (-gap.max(0.) / (radius * 1.1)).exp();
        let velocity = (b.vx - a.vx) * e[0] + (b.vy - a.vy) * e[1];
        let pull = strength * self.material.adhesion * influence * (velocity / 500.).tanh();
        let distance = self
            .material
            .rebound_limit
            .map_or(radius * 0.4, |limit| (radius * 0.4).min(limit * 0.5));
        let shift = distance * pull;
        let strain = 0.38 * pull * (e[0] * e[0] - e[1] * e[1]);
        let strain = self.material.rebound_limit.map_or(strain, |limit| {
            let bound = (1. + limit * 0.5 / rest.w.max(rest.h)).ln();
            bound * (strain / bound).tanh()
        });
        body.channels[0].target = rest.cx + (body.anchor[0] - 0.5) * rest.w + e[0] * shift;
        body.channels[1].target = rest.cy + (body.anchor[1] - 0.5) * rest.h + e[1] * shift;
        body.channels[2].target = rest.w * strain.exp();
        body.channels[3].target = rest.h * (-strain).exp();
        body.channels[4].target = rest.r;
    }
    pub fn tick(&mut self) {
        self.step(FIXED_DT);
    }
    fn step(&mut self, dt: f64) {
        self.dirty = false;
        if matches!(self.mode, Mode::Pair { .. }) {
            let before = [self.groups[0].body.pose(), self.groups[1].body.pose()];
            self.groups[1].body.step(dt, self.material);
            self.drive_source(1.);
            self.groups[0].body.step(dt, self.material);
            for (g, p) in self.groups.iter_mut().zip(before) {
                g.step(dt, self.material, Some(p));
            }
            let (a, b) = two_mut(&mut self.groups, 0, 1);
            let pa = a.prepared.pose;
            let pb = b.prepared.pose;
            let h = a.sigma.min(b.sigma) * 2.5;
            for p in &mut a.particles {
                for q in &mut b.particles {
                    let dx = pb.cx + q.x - pa.cx - p.x;
                    let dy = pb.cy + q.y - pa.cy - p.y;
                    let r = length([dx, dy]).max(0.001);
                    if r > h {
                        continue;
                    }
                    let rest = length([pb.cx + q.qx - pa.cx - p.qx, pb.cy + q.qy - pa.cy - p.qy]);
                    let d = ((rest - r) * dt * dt * 260. * self.material.adhesion * (1. - r / h))
                        .clamp(-0.25, 0.25);
                    let sum = p.mass + q.mass;
                    let x = dx / r * d;
                    let y = dy / r * d;
                    p.x -= x * q.mass / sum;
                    p.y -= y * q.mass / sum;
                    q.x += x * p.mass / sum;
                    q.y += y * p.mass / sum;
                    p.vx -= x * q.mass / sum / dt;
                    p.vy -= y * q.mass / sum / dt;
                    q.vx += x * p.mass / sum / dt;
                    q.vy += y * p.mass / sum / dt;
                }
            }
            for g in &mut self.groups {
                g.max_error = g
                    .particles
                    .iter()
                    .map(|p| length([p.x - p.qx, p.y - p.qy]))
                    .fold(0., f64::max);
                g.max_speed = g
                    .particles
                    .iter()
                    .map(|p| length([p.vx, p.vy]))
                    .fold(0., f64::max);
            }
        } else {
            let compound = self.is_compound();
            for g in &mut self.groups {
                // Compound members have independent dynamics. An unchanged
                // composer body need not run DDR while only a bubble moves.
                // Pair coupling above still advances both members together.
                if !compound || g.moving() {
                    g.step(dt, self.material, None);
                }
            }
        }
        self.revision += 1;
    }
    /// Rendering cadence does not change integration. Long suspensions are
    /// bounded to 12 substeps; hidden views should stop calling this method.
    pub fn advance(&mut self, elapsed: f64, reduced: bool) -> usize {
        if reduced {
            self.finish();
            return 0;
        }
        if !elapsed.is_finite() || elapsed <= 0. {
            return 0;
        }
        self.accumulator += elapsed.min(0.05);
        let steps = ((self.accumulator / FIXED_DT) + 1e-9).floor() as usize;
        self.accumulator = (self.accumulator - steps as f64 * FIXED_DT).max(0.);
        for _ in 0..steps {
            if self.moving() {
                self.tick();
                if self.presentation_settled() {
                    self.finish();
                }
            }
        }
        steps
    }
    pub fn finish(&mut self) {
        self.dirty = false;
        self.drive_source(0.);
        for g in &mut self.groups {
            g.finish(self.material);
        }
        self.accumulator = 0.;
        self.revision += 1;
    }
}
