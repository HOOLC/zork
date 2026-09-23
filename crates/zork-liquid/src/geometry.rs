//! Smooth rectangle geometry. Circles use the same primitive with equal axes.
use std::{cell::RefCell, f64::consts::PI, sync::Arc};

pub type Point = [f64; 2];

/// Parallel inner corners retain the outer curve's inset, bounded by the row.
pub fn inset_radius(outer_radius: f64, inset: f64, width: f64, height: f64) -> f64 {
    (outer_radius - inset)
        .max(0.)
        .min(width.min(height).max(0.) / 2.)
}

/// A short row must fit its parallel corner without flattening against the parent.
pub fn row_inset(outer_radius: f64, minimum: f64, row_height: f64) -> f64 {
    minimum.max(outer_radius - row_height / 2.)
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pose {
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
    pub r: f64,
    #[serde(default)]
    pub vx: f64,
    #[serde(default)]
    pub vy: f64,
    #[serde(default)]
    pub vw: f64,
    #[serde(default)]
    pub vh: f64,
}

impl Pose {
    pub fn rect(x: f64, y: f64, w: f64, h: f64, radius: f64) -> Self {
        Self {
            cx: x + w / 2.,
            cy: y + h / 2.,
            w,
            h,
            r: radius,
            vx: 0.,
            vy: 0.,
            vw: 0.,
            vh: 0.,
        }
    }
    pub fn left(self) -> f64 {
        self.cx - self.w / 2.
    }
    pub fn top(self) -> f64 {
        self.cy - self.h / 2.
    }
    pub fn short(self) -> f64 {
        self.w.min(self.h)
    }
    pub fn is_valid(self) -> bool {
        [
            self.cx, self.cy, self.w, self.h, self.r, self.vx, self.vy, self.vw, self.vh,
        ]
        .into_iter()
        .all(f64::is_finite)
            && self.w >= 2.
            && self.h >= 2.
            && self.r >= 0.
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cubic {
    pub from: Point,
    pub c1: Point,
    pub c2: Point,
    pub to: Point,
}
impl Cubic {
    pub fn at(self, t: f64) -> Point {
        let s = 1. - t;
        std::array::from_fn(|i| {
            s * s * s * self.from[i]
                + 3. * s * s * t * self.c1[i]
                + 3. * s * t * t * self.c2[i]
                + t * t * t * self.to[i]
        })
    }
}

#[derive(Debug)]
pub(crate) struct Profile {
    pub sums: [f32; 129],
    pub normals: [f32; 129],
    pub corner_area: f64,
}
thread_local! { static PROFILES: RefCell<Vec<(u64, Arc<Profile>)>> = const { RefCell::new(Vec::new()) }; }

pub(crate) fn mix(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}
pub(crate) fn length(p: Point) -> f64 {
    // UI coordinates are bounded. Avoid hypot for every distance-field sample
    // while retaining its behavior outside the safe range for squaring.
    let magnitude = p[0].abs().max(p[1].abs());
    if magnitude == 0. || (1e-150..=1e150).contains(&magnitude) {
        (p[0] * p[0] + p[1] * p[1]).sqrt()
    } else {
        p[0].hypot(p[1])
    }
}
pub(crate) fn unit(p: Point) -> Point {
    let d = length(p).max(1e-20);
    [p[0] / d, p[1] / d]
}

// The three-cubic corner follows squircle-path-kit 1.0.0's rounded-polygon
// construction. Keep the upstream 5-decimal control-point precision used by
// the approved reference before preparing its radial distance lookup.
// The upstream notice is retained in ../SQUIRCLE-LICENSE.txt.
fn corner(s: f64, rounded: bool) -> Vec<Cubic> {
    let p = 1. + s;
    let beta = PI / 4. * s;
    let t = (beta / 2.).tan();
    let a = 2. * (p - (1. - t)) / 3.;
    let start = [1. + beta.cos(), 1. + beta.sin()];
    let end = [1. + beta.sin(), 1. + beta.cos()];
    let sweep = PI / 2. * (1. - s);
    let k = 4. / 3. * (sweep / 4.).tan();
    let mut segments = vec![Cubic {
        from: [2., 2. - p],
        c1: [2., 2. - p + a],
        c2: [2., 1. + t],
        to: start,
    }];
    if sweep.abs() > 1e-6 {
        segments.push(Cubic {
            from: start,
            c1: [start[0] - k * beta.sin(), start[1] + k * beta.cos()],
            c2: [end[0] + k * beta.cos(), end[1] - k * beta.sin()],
            to: end,
        });
    }
    segments.push(Cubic {
        from: end,
        c1: [1. + t, 2.],
        c2: [2. - p + a, 2.],
        to: [2. - p, 2.],
    });
    if rounded {
        for c in &mut segments {
            for p in [&mut c.from, &mut c.c1, &mut c.c2, &mut c.to] {
                for v in p {
                    *v = (*v * 100_000.).round() / 100_000.;
                }
            }
        }
    }
    if s < 1e-6 {
        segments.remove(0);
        segments.pop();
    }
    segments
}

fn profile(s: f64) -> Arc<Profile> {
    PROFILES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, p)) = cache.iter().find(|(key, _)| *key == s.to_bits()) {
            return p.clone();
        }
        let origin = 1. - s;
        let points: Vec<Point> = corner(s, true)
            .into_iter()
            .flat_map(|c| {
                (0..=12).map(move |i| {
                    let p = c.at(i as f64 / 12.);
                    [p[0] - origin, p[1] - origin]
                })
            })
            .collect();
        let mut p = Profile {
            sums: [0.; 129],
            normals: [0.; 129],
            corner_area: 0.,
        };
        let mut edge = 1;
        for i in 0..=128 {
            let t = i as f64 / 128.;
            while edge < points.len() - 1
                && points[edge][1] / (points[edge][0] + points[edge][1]) < t
            {
                edge += 1;
            }
            while edge < points.len() - 1
                && length([
                    points[edge][0] - points[edge - 1][0],
                    points[edge][1] - points[edge - 1][1],
                ]) < 1e-8
            {
                edge += 1;
            }
            let a = points[edge - 1];
            let b = points[edge];
            let v = [b[0] - a[0], b[1] - a[1]];
            let denominator = (1. - t) * v[1] - t * v[0];
            let u = if denominator.abs() > 1e-10 {
                ((t * a[0] - (1. - t) * a[1]) / denominator).clamp(0., 1.)
            } else {
                0.
            };
            p.sums[i] = (a[0] + a[1] + u * (v[0] + v[1])) as f32;
            p.normals[i] = (denominator.abs() / length(v).max(1e-8)) as f32;
        }
        let mut previous = [1. + s, 0.];
        for i in 1..=80 {
            let t = i as f64 / 80.;
            let x = t * 128.;
            let j = (x.floor() as usize).min(127);
            let sum = mix(p.sums[j] as f64, p.sums[j + 1] as f64, x - j as f64);
            let q = [(1. - t) * sum, t * sum];
            p.corner_area += (previous[0] * q[1] - q[0] * previous[1]) / 2.;
            previous = q;
        }
        let p = Arc::new(p);
        if cache.len() >= 256 {
            cache.remove(0);
        }
        cache.push((s.to_bits(), p.clone()));
        p
    })
}

#[derive(Clone, Debug)]
struct Rect {
    r: f64,
    extent: f64,
    profile: Option<Arc<Profile>>,
}
impl Rect {
    fn new(p: Pose, r: f64, s: f64) -> Self {
        let r = r.clamp(0., p.short() / 2.);
        let s = s.min((p.short() / (2. * r.max(0.001)) - 1.).max(0.));
        Self {
            r,
            extent: r * (1. + s),
            profile: if s > 0.001 && r > 0.1 {
                Some(profile(s))
            } else {
                None
            },
        }
    }
    fn distance(&self, p: Pose, x: f64, y: f64) -> f64 {
        let dx = (x - p.cx).abs();
        let dy = (y - p.cy).abs();
        if let Some(profile) = &self.profile {
            let qx = dx - (p.w / 2. - self.extent);
            let qy = dy - (p.h / 2. - self.extent);
            if qx <= 0. || qy <= 0. {
                qx.max(qy) - self.extent
            } else {
                let sum = qx + qy;
                let t = qy / sum * 128.;
                let i = (t.floor() as usize).min(127);
                let f = t - i as f64;
                (sum - mix(profile.sums[i] as f64, profile.sums[i + 1] as f64, f) * self.r)
                    * mix(profile.normals[i] as f64, profile.normals[i + 1] as f64, f)
            }
        } else {
            let qx = dx - (p.w / 2. - self.r);
            let qy = dy - (p.h / 2. - self.r);
            length([qx.max(0.), qy.max(0.)]) + qx.max(qy).min(0.) - self.r
        }
    }
    fn area(&self, p: Pose) -> f64 {
        match &self.profile {
            Some(profile) => {
                p.w * p.h - 4. * (self.extent * self.extent - profile.corner_area * self.r * self.r)
            }
            None => p.w * p.h - (4. - PI) * self.r * self.r,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Prepared {
    pub pose: Pose,
    rect: Rect,
    corners: Option<[Rect; 4]>,
}
impl Prepared {
    pub fn new(pose: Pose, smoothing: f64, corners: Option<[f64; 4]>) -> Self {
        Self {
            pose,
            rect: Rect::new(pose, pose.r, smoothing),
            corners: corners.map(|r| r.map(|r| Rect::new(pose, r, smoothing))),
        }
    }
    pub fn distance(&self, x: f64, y: f64) -> f64 {
        let rect = if let Some(corners) = &self.corners {
            &corners[if y < self.pose.cy {
                if x < self.pose.cx {
                    0
                } else {
                    1
                }
            } else if x < self.pose.cx {
                3
            } else {
                2
            }]
        } else {
            &self.rect
        };
        rect.distance(self.pose, x, y)
    }
    pub fn radial(&self, a: f64) -> f64 {
        let (mut lo, mut hi) = (0., length([self.pose.w, self.pose.h]));
        let (sin, cos) = a.sin_cos();
        for _ in 0..18 {
            let m = (lo + hi) / 2.;
            if self.distance(self.pose.cx + cos * m, self.pose.cy + sin * m) < 0. {
                lo = m;
            } else {
                hi = m;
            }
        }
        (lo + hi) / 2.
    }
    pub fn area(&self) -> f64 {
        self.corners.as_ref().map_or_else(
            || self.rect.area(self.pose),
            |corners| corners.iter().map(|r| r.area(self.pose)).sum::<f64>() / 4.,
        )
    }
}

pub(crate) fn fit_area(mut p: Pose, area: f64, smoothing: f64) -> Pose {
    let scale = (area / Prepared::new(p, smoothing, None).area()).sqrt();
    p.w *= scale;
    p.h *= scale;
    p.r *= scale;
    p
}

/// Static controls and animated carriers share this smooth rectangle primitive.
pub fn rounded_rectangle(p: Pose, smoothing: f64) -> Vec<Cubic> {
    let r = p.r.clamp(0., p.short() / 2.);
    let smoothing = smoothing.min((p.short() / (2. * r.max(0.001)) - 1.).max(0.));
    let mut curves = Vec::new();
    if r < 1e-6 {
        let q = [
            [p.left(), p.top()],
            [p.left() + p.w, p.top()],
            [p.left() + p.w, p.top() + p.h],
            [p.left(), p.top() + p.h],
        ];
        return (0..4)
            .map(|i| Cubic {
                from: q[i],
                c1: q[i],
                c2: q[(i + 1) % 4],
                to: q[(i + 1) % 4],
            })
            .collect();
    }
    let template = corner(smoothing, false);
    for i in 0..4 {
        let map = |q: Point| {
            let [x, y] = match i {
                0 => [(2. - q[0]) * r, (2. - q[1]) * r],
                1 => [p.w - (2. - q[1]) * r, (2. - q[0]) * r],
                2 => [p.w - (2. - q[0]) * r, p.h - (2. - q[1]) * r],
                _ => [(2. - q[1]) * r, p.h - (2. - q[0]) * r],
            };
            [
                (x * 100_000.).round() / 100_000. + p.left(),
                (y * 100_000.).round() / 100_000. + p.top(),
            ]
        };
        let start = map(template[0].from);
        if let Some(previous) = curves.last().map(|c: &Cubic| c.to) {
            curves.push(Cubic {
                from: previous,
                c1: previous,
                c2: start,
                to: start,
            });
        }
        curves.extend(template.iter().map(|c| Cubic {
            from: map(c.from),
            c1: map(c.c1),
            c2: map(c.c2),
            to: map(c.to),
        }));
    }
    let from = curves.last().unwrap().to;
    let to = curves[0].from;
    curves.push(Cubic {
        from,
        c1: from,
        c2: to,
        to,
    });
    curves
}
fn contact_outline(p: Pose) -> Vec<Point> {
    let curves = rounded_rectangle(p, 0.6);
    let mut points = vec![curves[0].from];
    for c in curves {
        if c.c1 == c.from && c.c2 == c.to {
            points.push(c.to);
        } else {
            for i in 1..=8 {
                points.push(c.at(i as f64 / 8.));
            }
        }
    }
    points
}
pub(crate) fn contact_axis(a: Pose, b: Pose) -> Point {
    let delta = [b.cx - a.cx, b.cy - a.cy];
    let fallback = if length(delta) > 0.001 {
        unit(delta)
    } else {
        [0., 1.]
    };
    if delta[0].abs() <= (a.w + b.w) / 2. && delta[1].abs() <= (a.h + b.h) / 2. {
        return fallback;
    }
    let a = contact_outline(a);
    let b = contact_outline(b);
    let mut best = f64::INFINITY;
    let mut vector = fallback;
    for (vertices, segments, sign) in [(&a, &b, 1.), (&b, &a, -1.)] {
        for p in vertices {
            for i in 0..segments.len() {
                let s = segments[i];
                let e = segments[(i + 1) % segments.len()];
                let v = [e[0] - s[0], e[1] - s[1]];
                let squared = v[0] * v[0] + v[1] * v[1];
                if squared < 1e-12 {
                    continue;
                }
                let t = (((p[0] - s[0]) * v[0] + (p[1] - s[1]) * v[1]) / squared).clamp(0., 1.);
                let q = [
                    (s[0] + t * v[0] - p[0]) * sign,
                    (s[1] + t * v[1] - p[1]) * sign,
                ];
                let d = q[0] * q[0] + q[1] * q[1];
                if d < best - 1e-10 {
                    best = d;
                    vector = q;
                }
            }
        }
    }
    if best < 1e-8 || !best.is_finite() {
        fallback
    } else {
        unit(vector).map(|x| if x.abs() < 1e-8 { 0. } else { x })
    }
}
