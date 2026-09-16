//! A single fitted cubic path serves paint, content clipping and pointer tests.
use super::{
    field::{Grid, Sampler},
    geometry::{length, unit, Cubic, Point},
    physics::Simulation,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Clone, Debug)]
pub struct Contour {
    pub loops: Vec<Vec<Cubic>>,
    pub sampled_points: usize,
    pub full_grid_points: usize,
    pub used_fallback: bool,
    pub revision: u64,
}
impl Contour {
    pub fn contains(&self, point: Point) -> bool {
        let mut inside = false;
        for c in self.loops.iter().flatten() {
            // Split at vertical extrema, so every interval has at most one
            // crossing. This tests the fitted path rather than its old samples.
            let a = -c.from[1] + 3. * c.c1[1] - 3. * c.c2[1] + c.to[1];
            let b = 3. * c.from[1] - 6. * c.c1[1] + 3. * c.c2[1];
            let d = -3. * c.from[1] + 3. * c.c1[1];
            // A cubic has at most two vertical extrema. Hit tests and border
            // preparation run this for every segment; keep the cuts on stack.
            let mut cuts = [0., 1., 0., 0.];
            let mut count = 2;
            if a.abs() < 1e-12 {
                if b.abs() > 1e-12 {
                    let t = -d / (2. * b);
                    if t > 0. && t < 1. {
                        cuts[count] = t;
                        count += 1;
                    }
                }
            } else {
                let discriminant = 4. * b * b - 12. * a * d;
                if discriminant >= 0. {
                    for t in [
                        (-2. * b - discriminant.sqrt()) / (6. * a),
                        (-2. * b + discriminant.sqrt()) / (6. * a),
                    ] {
                        if t > 0. && t < 1. {
                            cuts[count] = t;
                            count += 1;
                        }
                    }
                }
            }
            cuts[..count].sort_by(f64::total_cmp);
            for pair in cuts[..count].windows(2) {
                let (mut lo, mut hi) = (pair[0], pair[1]);
                let y0 = c.at(lo)[1];
                let y1 = c.at(hi)[1];
                if (y0 > point[1]) == (y1 > point[1]) {
                    continue;
                }
                for _ in 0..28 {
                    let middle = (lo + hi) / 2.;
                    if (c.at(middle)[1] > point[1]) == (y0 > point[1]) {
                        lo = middle;
                    } else {
                        hi = middle;
                    }
                }
                if c.at((lo + hi) / 2.)[0] > point[0] {
                    inside = !inside;
                }
            }
        }
        inside
    }
    pub fn svg_path(&self) -> String {
        use std::fmt::Write;
        let mut output = String::new();
        for curves in &self.loops {
            if let Some(first) = curves.first() {
                let _ = write!(output, "M{:.3},{:.3}", first.from[0], first.from[1]);
                for c in curves {
                    let _ = write!(
                        output,
                        "C{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
                        c.c1[0], c.c1[1], c.c2[0], c.c2[1], c.to[0], c.to[1]
                    );
                }
                output.push('Z');
            }
        }
        output
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContourError {
    Bounds,
    MissingSeed,
    OpenContour,
    Topology,
}

#[derive(Default)]
struct BoundaryNode {
    point: Point,
    neighbors: [usize; 2],
    degree: u8,
    seen: bool,
}

/// Per-surface scratch storage. Internal integer grid keys do not need a
/// randomized hash; retain allocations between frames instead of rebuilding
/// five maps and a heap-allocated neighbor list for every boundary vertex.
#[derive(Default)]
pub(crate) struct Workspace {
    values: HashMap<usize, f64>,
    visited: HashSet<usize>,
    nodes: HashMap<usize, BoundaryNode>,
    order: Vec<usize>,
    stack: Vec<usize>,
    points: Vec<Point>,
    fit_params: Vec<f64>,
}
impl Workspace {
    fn clear(&mut self) {
        self.values.clear();
        self.visited.clear();
        self.nodes.clear();
        self.order.clear();
        self.stack.clear();
        self.points.clear();
    }
}
struct Tracer<'a> {
    grid: Grid,
    sampler: Sampler<'a>,
    workspace: Workspace,
}
impl<'a> Tracer<'a> {
    fn new(
        s: &'a Simulation,
        sparse: bool,
        mut workspace: Workspace,
    ) -> Result<Self, ContourError> {
        let grid = Grid::for_simulation(s, sparse);
        if grid
            .width
            .checked_mul(grid.height)
            .is_none_or(|n| n > 4_000_000)
            || grid.width < 2
            || grid.height < 2
        {
            return Err(ContourError::Bounds);
        }
        workspace.clear();
        Ok(Self {
            grid,
            sampler: Sampler::new(s),
            workspace,
        })
    }
    fn value(&mut self, x: usize, y: usize) -> f64 {
        let i = y * self.grid.width + x;
        *self.workspace.values.entry(i).or_insert_with(|| {
            self.sampler.value(
                self.grid.x + x as f64 * self.grid.cell,
                self.grid.y + y as f64 * self.grid.cell,
            )
        })
    }
    fn add(&mut self, x: usize, y: usize) {
        if x < self.grid.width - 1 && y < self.grid.height - 1 {
            self.workspace.stack.push(y * self.grid.width + x);
        }
    }
    fn link(&mut self, a: usize, b: usize) {
        for (x, y) in [(a, b), (b, a)] {
            let node = self.workspace.nodes.get_mut(&x).unwrap();
            if node.degree == 0 {
                self.workspace.order.push(x);
            }
            if node.degree < 2 {
                node.neighbors[node.degree as usize] = y;
            }
            node.degree = (node.degree + 1).min(3);
        }
    }
    fn visit(&mut self, x: usize, y: usize) {
        let width = self.grid.width;
        let i = y * width + x;
        if !self.workspace.visited.insert(i) {
            return;
        }
        let q = [
            self.value(x, y),
            self.value(x + 1, y),
            self.value(x + 1, y + 1),
            self.value(x, y + 1),
        ];
        self.visit_values(x, y, q, true);
    }
    fn visit_values(&mut self, x: usize, y: usize, q: [f64; 4], sparse: bool) {
        let width = self.grid.width;
        let i = y * width + x;
        let positive = q.map(|v| v >= 0.);
        if positive.iter().all(|p| *p) || positive.iter().all(|p| !*p) {
            return;
        }
        let corners = [[x, y], [x + 1, y], [x + 1, y + 1], [x, y + 1]];
        let mut ids = [0; 4];
        let mut count = 0;
        for e in 0..4 {
            let next = (e + 1) % 4;
            if positive[e] == positive[next] {
                continue;
            }
            let key = match e {
                0 => i * 2,
                1 => (i + 1) * 2 + 1,
                2 => (i + width) * 2,
                _ => i * 2 + 1,
            };
            let a = corners[e];
            let b = corners[next];
            let t = -q[e] / (q[next] - q[e]);
            let cell = self.grid.cell;
            self.workspace.nodes.entry(key).or_default().point = [
                self.grid.x + (a[0] as f64 + (b[0] as f64 - a[0] as f64) * t) * cell,
                self.grid.y + (a[1] as f64 + (b[1] as f64 - a[1] as f64) * t) * cell,
            ];
            ids[count] = key;
            count += 1;
            if sparse {
                match e {
                    0 => {
                        if y > 0 {
                            self.add(x, y - 1);
                        }
                    }
                    1 => self.add(x + 1, y),
                    2 => self.add(x, y + 1),
                    _ => {
                        if x > 0 {
                            self.add(x - 1, y);
                        }
                    }
                }
            }
        }
        if count == 2 {
            self.link(ids[0], ids[1]);
        } else if count == 4 {
            if q.iter().sum::<f64>() >= 0. {
                self.link(ids[0], ids[1]);
                self.link(ids[2], ids[3]);
            } else {
                self.link(ids[0], ids[3]);
                self.link(ids[1], ids[2]);
            }
        }
    }
    fn trace(&mut self, s: &Simulation, sparse: bool) -> Result<Contour, ContourError> {
        if sparse {
            for group in &s.groups {
                let (mut x, y) = self.grid.locate(group.prepared.pose);
                if x >= self.grid.width || y >= self.grid.height || self.value(x, y) < 0. {
                    return Err(ContourError::MissingSeed);
                }
                while x < self.grid.width - 1 && self.value(x + 1, y) >= 0. {
                    x += 1;
                }
                if x >= self.grid.width - 1 {
                    return Err(ContourError::Bounds);
                }
                self.add(x, y);
                if y > 0 {
                    self.add(x, y - 1);
                }
                while let Some(i) = self.workspace.stack.pop() {
                    self.visit(i % self.grid.width, i / self.grid.width);
                }
            }
        } else {
            // A dense fallback scans each cell exactly once. Cache two rows,
            // not a hash entry for every sample and every visited interior cell.
            // Only boundary links survive beyond a row (also bounds wasm memory).
            let row = |y: usize, sampler: &Sampler<'_>, grid: Grid| {
                (0..grid.width)
                    .map(|x| {
                        sampler.value(grid.x + x as f64 * grid.cell, grid.y + y as f64 * grid.cell)
                    })
                    .collect::<Vec<_>>()
            };
            let mut top = row(0, &self.sampler, self.grid);
            for y in 0..self.grid.height - 1 {
                let bottom = row(y + 1, &self.sampler, self.grid);
                for x in 0..self.grid.width - 1 {
                    self.visit_values(x, y, [top[x], top[x + 1], bottom[x + 1], bottom[x]], false);
                }
                top = bottom;
            }
        }
        let mut loops = Vec::new();
        for start in &self.workspace.order {
            if self.workspace.nodes[start].seen {
                continue;
            }
            let mut current = *start;
            let mut previous = None;
            self.workspace.points.clear();
            loop {
                let node = self.workspace.nodes.get_mut(&current).unwrap();
                if node.seen {
                    if current != *start {
                        return Err(ContourError::OpenContour);
                    }
                    break;
                }
                node.seen = true;
                self.workspace.points.push(node.point);
                let adjacent = node.neighbors;
                if node.degree != 2 {
                    return Err(ContourError::OpenContour);
                }
                let next = if Some(adjacent[0]) != previous {
                    adjacent[0]
                } else {
                    adjacent[1]
                };
                previous = Some(current);
                current = next;
            }
            if self.workspace.points.len() > 5 {
                loops.push(fit_closed(
                    &self.workspace.points,
                    0.045,
                    &mut self.workspace.fit_params,
                ));
            }
        }
        if loops.is_empty() || (sparse && loops.len() > s.groups.len()) {
            return Err(ContourError::Topology);
        }
        Ok(Contour {
            loops,
            sampled_points: if sparse {
                self.workspace.values.len()
            } else {
                self.grid.width * self.grid.height
            },
            full_grid_points: self.grid.width * self.grid.height,
            used_fallback: !sparse,
            revision: s.revision,
        })
    }
}

pub fn trace(s: &Simulation) -> Result<Contour, ContourError> {
    trace_with_workspace(s, &mut Workspace::default())
}
pub(crate) fn trace_with_workspace(
    s: &Simulation,
    workspace: &mut Workspace,
) -> Result<Contour, ContourError> {
    if s.material.verified_sparse() {
        if let Ok(contour) = trace_once(s, true, workspace) {
            return Ok(contour);
        }
    }
    trace_once(s, false, workspace)
}
pub fn trace_dense(s: &Simulation) -> Result<Contour, ContourError> {
    trace_once(s, false, &mut Workspace::default())
}
fn trace_once(
    s: &Simulation,
    sparse: bool,
    workspace: &mut Workspace,
) -> Result<Contour, ContourError> {
    let mut tracer = Tracer::new(s, sparse, std::mem::take(workspace))?;
    let result = tracer.trace(s, sparse);
    *workspace = tracer.workspace;
    result
}

fn fit_closed(points: &[Point], tolerance: f64, params: &mut Vec<f64>) -> Vec<Cubic> {
    let n = points.len();
    let at = |i: usize| points[i % n];
    let mut out = Vec::new();
    if n < 8 {
        for i in 0..n {
            let a = at(i + n - 1);
            let b = at(i);
            let c = at(i + 1);
            let d = at(i + 2);
            out.push(Cubic {
                from: b,
                c1: [b[0] + (c[0] - a[0]) / 6., b[1] + (c[1] - a[1]) / 6.],
                c2: [c[0] - (d[0] - b[0]) / 6., c[1] - (d[1] - b[1]) / 6.],
                to: c,
            });
        }
        return out;
    }
    for pair in [0, n / 4, n / 2, 3 * n / 4, n].windows(2) {
        fit(points, pair[0], pair[1], tolerance, 0, &mut out, params);
    }
    out
}
fn fit(
    points: &[Point],
    first: usize,
    last: usize,
    tolerance: f64,
    depth: usize,
    out: &mut Vec<Cubic>,
    params: &mut Vec<f64>,
) {
    let n = points.len();
    let at = |i: usize| points[i % n];
    let tangent = |i: usize| {
        let a = at(i + n - 1);
        let b = at(i + 1);
        unit([b[0] - a[0], b[1] - a[1]])
    };
    let p = at(first);
    let q = at(last);
    let a = tangent(first);
    let b = tangent(last);
    let count = last - first;
    // Recursive halves run sequentially, so one retained parameter buffer
    // serves every fit without changing the order of floating-point work.
    params.resize(count + 1, 0.);
    params[0] = 0.;
    let mut total = 0.;
    for (i, v) in params.iter_mut().enumerate().skip(1) {
        let c = at(first + i);
        let d = at(first + i - 1);
        total += length([c[0] - d[0], c[1] - d[1]]);
        *v = total;
    }
    if total < 1e-9 {
        return;
    }
    let (mut aa, mut ab, mut bb, mut ad, mut bd) = (0., 0., 0., 0., 0.);
    for (i, param) in params.iter().enumerate().take(count).skip(1) {
        let t = param / total;
        let s = 1. - t;
        let b0 = s * s * s;
        let b1 = 3. * s * s * t;
        let b2 = 3. * s * t * t;
        let b3 = t * t * t;
        let c = at(first + i);
        let dx = c[0] - (b0 + b1) * p[0] - (b2 + b3) * q[0];
        let dy = c[1] - (b0 + b1) * p[1] - (b2 + b3) * q[1];
        let ax = b1 * a[0];
        let ay = b1 * a[1];
        let bx = -b2 * b[0];
        let by = -b2 * b[1];
        aa += ax * ax + ay * ay;
        ab += ax * bx + ay * by;
        bb += bx * bx + by * by;
        ad += ax * dx + ay * dy;
        bd += bx * dx + by * dy;
    }
    let determinant = aa * bb - ab * ab;
    let mut alpha = if determinant > 1e-12 {
        (ad * bb - bd * ab) / determinant
    } else {
        total / 3.
    };
    let mut beta = if determinant > 1e-12 {
        (bd * aa - ad * ab) / determinant
    } else {
        total / 3.
    };
    if alpha < total * 1e-6 || beta < total * 1e-6 || alpha > total * 3. || beta > total * 3. {
        alpha = total / 3.;
        beta = total / 3.;
    }
    let c = Cubic {
        from: p,
        c1: [p[0] + alpha * a[0], p[1] + alpha * a[1]],
        c2: [q[0] - beta * b[0], q[1] - beta * b[1]],
        to: q,
    };
    let mut error = 0.;
    let mut split = count / 2;
    for (i, param) in params.iter().enumerate().take(count).skip(1) {
        let q = c.at(param / total);
        let p = at(first + i);
        let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
        if d > error {
            error = d;
            split = i;
        }
    }
    if error > tolerance * tolerance && count > 2 && depth < 16 {
        fit(
            points,
            first,
            first + split,
            tolerance,
            depth + 1,
            out,
            params,
        );
        fit(
            points,
            first + split,
            last,
            tolerance,
            depth + 1,
            out,
            params,
        );
    } else {
        out.push(c);
    }
}
