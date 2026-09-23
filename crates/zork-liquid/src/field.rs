//! Reference-calibrated signed field; evaluate only points requested by tracing.
use super::{
    geometry::{length, Pose},
    physics::{Group, Simulation},
};
use std::sync::OnceLock;

fn f32_value(x: f64) -> f64 {
    x as f32 as f64
}
fn kernel_table() -> &'static [f32; 4097] {
    static EXP: OnceLock<[f32; 4097]> = OnceLock::new();
    EXP.get_or_init(|| std::array::from_fn(|i| (-(i as f64) / 512.).exp() as f32))
}
fn kernel(table: &[f32; 4097], dx: f64, dy: f64, inverse: f64) -> f64 {
    let (x, y) = (dx * inverse, dy * inverse);
    if x.abs().max(y.abs()) > 3.3 {
        return 0.;
    }
    // Nonnegative and bounded by the support check above. Truncation plus the
    // fractional comparison is the same tie-away rounding without libm's
    // software round call in the inner loop.
    let index = (x * x + y * y) * 256.;
    let whole = index as usize;
    let i = whole + usize::from(index - whole as f64 >= 0.5);
    if i >= 4097 {
        0.
    } else {
        table[i] as f64
    }
}

#[derive(Clone, Copy, Default)]
struct Fit {
    qx: f64,
    qy: f64,
    ux: f64,
    uy: f64,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
}
fn deformation_fit(group: &Group) -> Fit {
    let mass = group.mass();
    if mass < 1e-8 {
        return Fit::default();
    }
    let mut f = Fit::default();
    for p in &group.particles {
        let w = p.mass / mass;
        f.qx += w * p.qx;
        f.qy += w * p.qy;
        f.ux += w * (p.x - p.qx);
        f.uy += w * (p.y - p.qy);
    }
    let sx = (group.prepared.pose.w / 2.).max(1.);
    let sy = (group.prepared.pose.h / 2.).max(1.);
    let (mut xx, mut xy, mut yy, mut uxx, mut uxy, mut uyx, mut uyy) =
        (0.0001, 0., 0.0001, 0., 0., 0., 0.);
    for p in &group.particles {
        let w = p.mass / mass;
        let x = (p.qx - f.qx) / sx;
        let y = (p.qy - f.qy) / sy;
        let dx = p.x - p.qx - f.ux;
        let dy = p.y - p.qy - f.uy;
        xx += w * x * x;
        xy += w * x * y;
        yy += w * y * y;
        uxx += w * dx * x;
        uxy += w * dx * y;
        uyx += w * dy * x;
        uyy += w * dy * y;
    }
    let det = xx * yy - xy * xy;
    f.a = ((uxx * yy - uxy * xy) / det / sx).clamp(-0.6, 0.6);
    f.b = ((uxy * xx - uxx * xy) / det / sy).clamp(-0.6, 0.6);
    f.c = ((uyx * yy - uyy * xy) / det / sx).clamp(-0.6, 0.6);
    f.d = ((uyy * xx - uyx * xy) / det / sy).clamp(-0.6, 0.6);
    f
}
struct Parcel {
    x: f64,
    y: f64,
    rx: f64,
    ry: f64,
    inverse: f64,
    weight: f64,
    strength: f64,
    ux: f64,
    uy: f64,
}

struct ParcelRows {
    origin: f64,
    step: f64,
    buckets: Vec<Vec<usize>>,
}
impl ParcelRows {
    fn new(parcels: &[Parcel], sigma: f64) -> Option<Self> {
        let supports: Vec<_> = parcels
            .iter()
            .map(|p| {
                let reach = 3.3 / p.inverse;
                // Coarse selection is conservative; kernel() remains authoritative
                // at the exact support boundary, including floating-point rounding.
                let margin = (p.y.abs().max(p.ry.abs()) + reach).max(1.) * 1e-12;
                (
                    p.y.min(p.ry) - reach - margin,
                    p.y.max(p.ry) + reach + margin,
                )
            })
            .collect();
        let origin = supports.iter().map(|s| s.0).fold(f64::INFINITY, f64::min);
        let end = supports
            .iter()
            .map(|s| s.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let step = (sigma * 2.).max(1.);
        let count = ((end - origin) / step).ceil() + 1.;
        if !count.is_finite() || !(1. ..=4096.).contains(&count) {
            return None;
        }
        let mut buckets = vec![Vec::new(); count as usize];
        // Preserve parcel order: the reference field rounds after each parcel.
        for (index, (start, end)) in supports.into_iter().enumerate() {
            let first = ((start - origin) / step).floor() as usize;
            let last = (((end - origin) / step).floor() as usize).min(buckets.len() - 1);
            for bucket in &mut buckets[first..=last] {
                bucket.push(index);
            }
        }
        Some(Self {
            origin,
            step,
            buckets,
        })
    }
    fn at(&self, y: f64) -> &[usize] {
        if y < self.origin {
            return &[];
        }
        self.buckets
            .get(((y - self.origin) / self.step).floor() as usize)
            .map_or(&[], Vec::as_slice)
    }
}

pub(crate) struct Sampler<'a> {
    simulation: &'a Simulation,
    sigma: f64,
    gradient_cell: f64,
    mode: [f64; 2],
    amount: f64,
    fit: Option<Fit>,
    blend: f64,
    parcels: Vec<Parcel>,
    parcel_ends: Vec<usize>,
    parcel_indices: Vec<usize>,
    parcel_rows: Option<ParcelRows>,
    kernel_table: &'static [f32; 4097],
}
impl<'a> Sampler<'a> {
    pub fn new(simulation: &'a Simulation) -> Self {
        let groups = &simulation.groups;
        let small = groups
            .iter()
            .map(|g| g.prepared.pose.short())
            .fold(f64::INFINITY, f64::min);
        let sigma = groups.iter().map(|g| g.sigma).fold(0., f64::max);
        let affine = groups.len() == 1 && groups[0].affine;
        let mode = if affine {
            groups[0].axial.map(|s| s.position)
        } else {
            [0., 0.]
        };
        let amount = (mode[0] + mode[1]).min(1.);
        let fit = (amount > 0.000001).then(|| deformation_fit(&groups[0]));
        let m = simulation.material;
        let separation = if groups.len() > 1 {
            let a = groups[0].prepared.pose;
            let b = groups[1].prepared.pose;
            length([a.cx - b.cx, a.cy - b.cy])
        } else {
            0.
        };
        let speed = groups
            .iter()
            .map(|g| {
                let p = g.prepared.pose;
                p.vx * p.vx + p.vy * p.vy + 0.25 * (p.vw * p.vw + p.vh * p.vh)
            })
            .sum::<f64>()
            .sqrt();
        let activity = ((speed - 5.) / 175.).clamp(0., 1.);
        let blend = simulation.fusion
            + small
                * m.fusion_gain
                * m.adhesion
                * (1. - (-(separation / (small * 0.7).max(1.)).powi(2)).exp())
                * activity
                * activity
                * (3. - 2. * activity);
        let mut parcels = Vec::with_capacity(groups.iter().map(|g| g.particles.len()).sum());
        let mut parcel_ends = vec![0];
        for g in groups {
            let average = g.mass() / g.effective_count;
            let amplitude = 0.8 * g.sigma / sigma * g.coverage * m.surface_detail;
            let f = fit.unwrap_or_default();
            for q in &g.particles {
                // At rest these two kernels cancel. Do not evaluate them for
                // static compound members on every contour sample.
                if simulation.is_compound() && q.x == q.qx && q.y == q.qy {
                    continue;
                }
                let x = q.qx - f.qx;
                let y = q.qy - f.qy;
                parcels.push(Parcel {
                    x: g.prepared.pose.cx + q.x,
                    y: g.prepared.pose.cy + q.y,
                    rx: g.prepared.pose.cx + q.qx,
                    ry: g.prepared.pose.cy + q.qy,
                    inverse: 1. / g.sigma,
                    weight: q.mass / average,
                    strength: q.mass / average * amplitude,
                    ux: q.x - q.qx - f.ux - f.a * x - f.b * y,
                    uy: q.y - q.qy - f.uy - f.c * x - f.d * y,
                });
            }
            parcel_ends.push(parcels.len());
        }
        let parcel_indices = (0..parcels.len()).collect();
        let parcel_rows = (!simulation.is_compound())
            .then(|| ParcelRows::new(&parcels, sigma))
            .flatten();
        Self {
            simulation,
            sigma,
            gradient_cell: (small / 24.).clamp(0.4, 2.6),
            mode,
            amount,
            fit,
            blend,
            parcels,
            parcel_ends,
            parcel_indices,
            parcel_rows,
            kernel_table: kernel_table(),
        }
    }
    /// Compound members deform their own distance field before union. A moving
    /// member must not widen the union kernel of distant, stationary parcels.
    fn compound_value(&self, x: f64, y: f64) -> f64 {
        let fusion = self.simulation.fusion;
        let mut merged = f64::INFINITY;
        for (i, group) in self.simulation.groups.iter().enumerate() {
            let distance = group.prepared.distance(x, y);
            let reach = group.prepared.pose.short() * 0.24;
            // The bounded deformation cannot bring this member close enough to
            // affect the current surface. Also bounds work on large compounds.
            if distance - reach > merged + fusion {
                continue;
            }
            let base = f32_value(-distance / self.sigma);
            let mut value = base;
            for p in &self.parcels[self.parcel_ends[i]..self.parcel_ends[i + 1]] {
                let rest = kernel(self.kernel_table, x - p.rx, y - p.ry, p.inverse);
                let current = kernel(self.kernel_table, x - p.x, y - p.y, p.inverse);
                value = f32_value(value - p.strength * rest);
                value = f32_value(value + p.strength * current);
            }
            let limit = f32_value(reach / self.sigma);
            let distance = -(base + limit * ((value - base) / limit).tanh()) * self.sigma;
            let blend = if fusion > 0. {
                (fusion - (merged - distance).abs()).max(0.) / fusion
            } else {
                0.
            };
            merged = merged.min(distance) - blend * blend * fusion * 0.25;
        }
        f32_value(-merged / self.sigma)
    }
    pub fn value(&self, x: f64, y: f64) -> f64 {
        if self.simulation.is_compound() {
            return self.compound_value(x, y);
        }
        let groups = &self.simulation.groups;
        let mut nearest = groups[0].prepared.distance(x, y);
        let mut d = nearest;
        let mut owner = 0;
        for (i, g) in groups.iter().enumerate().skip(1) {
            let b = g.prepared.distance(x, y);
            let h = if self.blend > 0. {
                (self.blend - (d - b).abs()).max(0.) / self.blend
            } else {
                0.
            };
            if b < nearest {
                nearest = b;
                owner = i;
            }
            d = d.min(b) - h * h * self.blend * 0.25;
        }
        let base = f32_value(-d / self.sigma);
        let limit = f32_value(groups[owner].prepared.pose.short() * 0.24 / self.sigma);
        let (mut value, mut weight, mut ux, mut uy) = (base, 0., 0., 0.);
        let indices = self
            .parcel_rows
            .as_ref()
            .map_or(self.parcel_indices.as_slice(), |rows| rows.at(y));
        for &index in indices {
            let p = &self.parcels[index];
            let r = kernel(self.kernel_table, x - p.rx, y - p.ry, p.inverse);
            let a = kernel(self.kernel_table, x - p.x, y - p.y, p.inverse);
            value = f32_value(value - p.strength * r);
            value = f32_value(value + p.strength * a);
            if self.fit.is_some() {
                let w = p.weight * r;
                weight = f32_value(weight + w);
                ux = f32_value(ux + w * p.ux);
                uy = f32_value(uy + w * p.uy);
            }
        }
        let mut correction = value - base;
        if let Some(f) = self.fit {
            let group = &groups[0];
            let p = group.prepared.pose;
            let c = self.gradient_cell;
            let walls = group.constraints;
            let mut gx = (group.prepared.distance(x + c, y) - group.prepared.distance(x - c, y))
                / (2. * c * self.sigma);
            let mut gy = (group.prepared.distance(x, y + c) - group.prepared.distance(x, y - c))
                / (2. * c * self.sigma);
            if (walls.left && !walls.right && gx < 0.) || (walls.right && !walls.left && gx > 0.) {
                gx = 0.;
            }
            if (walls.top && !walls.bottom && gy < 0.) || (walls.bottom && !walls.top && gy > 0.) {
                gy = 0.;
            }
            let qx = x - p.cx - f.qx;
            let qy = y - p.cy - f.qy;
            let a = f.ux
                + f.a * qx
                + f.b * qy
                + if weight > 0.00001 {
                    0.35 * ux / weight
                } else {
                    0.
                };
            let b = f.uy
                + f.c * qx
                + f.d * qy
                + if weight > 0.00001 {
                    0.35 * uy / weight
                } else {
                    0.
                };
            let advection = self.simulation.material.surface_detail
                * ((2. + self.mode[0] / self.amount.max(0.00001)) * gx * a
                    + (2. + self.mode[1] / self.amount.max(0.00001)) * gy * b);
            correction = (1. - 0.45 * self.amount) * correction + 0.45 * self.amount * advection;
        }
        value = base + limit * (correction / limit).tanh();
        if groups.len() == 1 {
            let p = groups[0].prepared.pose;
            let c = groups[0].constraints;
            if c.left {
                value = value.min((x - p.left()) / self.sigma);
            }
            if c.right {
                value = value.min((p.left() + p.w - x) / self.sigma);
            }
            if c.top {
                value = value.min((y - p.top()) / self.sigma);
            }
            if c.bottom {
                value = value.min((p.top() + p.h - y) / self.sigma);
            }
        }
        f32_value(value)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Grid {
    pub x: f64,
    pub y: f64,
    pub width: usize,
    pub height: usize,
    pub cell: f64,
}
impl Grid {
    pub fn for_simulation(s: &Simulation, sparse: bool) -> Self {
        let small = s
            .groups
            .iter()
            .map(|g| g.prepared.pose.short())
            .fold(f64::INFINITY, f64::min);
        let original = (small / 24.).clamp(0.4, 2.6);
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for g in &s.groups {
            let p = g.prepared.pose;
            let pad = g.sigma * 2.8 + 4.;
            x0 = x0.min(p.left() - pad);
            y0 = y0.min(p.top() - pad);
            x1 = x1.max(p.left() + p.w + pad);
            y1 = y1.max(p.top() + p.h + pad);
        }
        let x = (x0 / original).floor() * original;
        let y = (y0 / original).floor() * original;
        let nx = ((x1 - x) / original).ceil() as usize + 1;
        let ny = ((y1 - y) / original).ceil() as usize + 1;
        if !sparse {
            return Self {
                x,
                y,
                width: nx,
                height: ny,
                cell: original,
            };
        }
        let cell = (small / 8.).clamp(0.4, 2.4);
        let ox = (x / cell).floor() * cell;
        let oy = (y / cell).floor() * cell;
        Self {
            x: ox,
            y: oy,
            width: ((x + nx as f64 * original - ox) / cell).ceil() as usize + 1,
            height: ((y + ny as f64 * original - oy) / cell).ceil() as usize + 1,
            cell,
        }
    }
    pub fn locate(self, p: Pose) -> (usize, usize) {
        (
            ((p.cx - self.x) / self.cell).round() as usize,
            ((p.cy - self.y) / self.cell).round() as usize,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Material, Options};

    #[test]
    fn parcel_rows_match_full_field_in_motion_and_at_support_boundaries() {
        let source = Pose::rect(-17.25, 9.5, 64., 32., 16.);
        let target = Pose::rect(110.75, 170.25, 540., 660., 24.);
        for paired in [false, true] {
            let mut simulation = if paired {
                Simulation::pair(source, target, Material::default(), Options::default())
            } else {
                Simulation::new(source, Material::default(), Options::default())
            };
            if paired {
                simulation.set_open(true);
            } else {
                simulation.set_target(target);
            }
            for step in 0..5 {
                for _ in 0..17 {
                    simulation.tick();
                }
                if step == 2 {
                    if paired {
                        simulation.set_open(false);
                    } else {
                        simulation.set_target(source);
                    }
                }
                let indexed = Sampler::new(&simulation);
                let mut full = Sampler::new(&simulation);
                full.parcel_rows = None;
                assert!(indexed.parcel_rows.is_some());
                let grid = Grid::for_simulation(&simulation, true);
                let mut ys: Vec<_> = (0..=40)
                    .map(|i| grid.y + i as f64 * grid.height as f64 * grid.cell / 40.)
                    .collect();
                for p in &indexed.parcels {
                    for center in [p.y, p.ry] {
                        for direction in [-1., 1.] {
                            let edge = center + direction * 3.3 / p.inverse;
                            ys.extend([edge - 1e-9, edge, edge + 1e-9]);
                        }
                    }
                }
                for y in ys {
                    for i in 0..=24 {
                        let x = grid.x + i as f64 * grid.width as f64 * grid.cell / 24.;
                        assert_eq!(
                            indexed.value(x, y),
                            full.value(x, y),
                            "paired={paired} step={step} x={x} y={y}"
                        );
                    }
                }
            }
        }
    }
}
