//! Shared GPUI liquid silhouette. Input remains the ordinary GPUI editor.
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};
mod field;
#[cfg(target_os = "macos")]
mod gpu;
pub(super) fn prepare_gpu() {
    #[cfg(target_os = "macos")]
    gpu::prepare();
}
#[cfg(all(target_os = "macos", feature = "headless-bench"))]
pub fn gpu_stats() -> (u64, f64) {
    gpu::stats()
}

// Approved in the interactive liquid tuner. Distances are logical pixels.
pub const RADIUS: f32 = 16.;
pub const SPACING: f32 = 22.;
pub const EDGE: f32 = 22.;
pub const ACTIVE_EDGE: f32 = 0.;
pub const IMMERSION: f32 = 8.;
pub const ROW_SPACING: f32 = 35.;
pub const DOCK_GAP: f32 = 3.;
pub const TOP_EXTENSION: f32 = 12.;
pub const EDITOR_TOP_INSET: f32 = 8.;
/// Equal side and bottom spacing keeps the action concentric with the outer corner.
pub const ACTION_INSET: f32 = 6.;
/// Outer corner follows the circular send button plus its bottom inset.
pub const SURFACE_RADIUS: f32 = crate::design::CUE_UI.composer.surface_radius;
pub const EDITOR_ACTION_GAP: f32 = 2.;
pub const COMPOSER_CHROME: f32 = EDITOR_TOP_INSET
    + EDITOR_ACTION_GAP
    + crate::design::CUE_UI.composer.action_size
    + ACTION_INSET;
pub const DEFAULT_HEIGHT: f32 = 24. + TOP_EXTENSION + COMPOSER_CHROME;
const MEMBER_FUSION: f32 = 4.;
const DOCK_FUSION: f32 = 4.;
pub const SURFACE_COLOR: u32 = 0xF6F5F1;
pub const LOWER_COLOR: u32 = SURFACE_COLOR;
pub const BORDER_COLOR: u32 = 0xDEDFDF;
pub const BORDER_WIDTH: f32 = crate::design::BORDER_WIDTH;
pub const SLOT_BORDER_WIDTH: f32 = crate::design::BORDER_WIDTH;
pub const TEXT_COLOR: u32 = 0x24272B;
pub const BUTTON_COLOR: u32 = 0x24282B;
pub const GLYPH_COLOR: u32 = 0xFFFFFF;

#[derive(Clone, Copy, PartialEq)]
pub struct Bubble {
    pub x: f32,
    pub lift: f32,
    pub width: f32,
}

/// A shallow right-side rise supporting a fan of file previews.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shelf {
    pub width: f32,
    pub height: f32,
    pub inset: f32,
    /// Position of the crown along the rise, between zero and one.
    pub peak: f32,
}
impl Shelf {
    fn rise(self, x: f32, width: f32) -> f32 {
        let t = ((x - (width - self.inset - self.width)) / self.width.max(1.)).clamp(0., 1.);
        let peak = self.peak.clamp(0.1, 0.9);
        let u = if t < peak {
            t / peak
        } else {
            (1. - t) / (1. - peak)
        };
        self.height * u * u * (3. - 2. * u)
    }
}

#[derive(Clone, Default)]
pub struct SurfaceCache(Rc<RefCell<Cache>>);
#[derive(Default)]
struct Cache {
    key: Option<(Bounds<Pixels>, f32, Vec<Bubble>, Option<Shelf>)>,
    path: Option<Path<Pixels>>,
    border: Option<Path<Pixels>>,
    attachment_key: Option<(Bounds<Pixels>, super::attachment_fan::Opening)>,
    attachment_path: Option<Path<Pixels>>,
    attachment_border: Option<Path<Pixels>>,
    attachment_hole_border: Option<Path<Pixels>>,
    attachment_gap: (f32, f32),
    #[cfg(target_os = "macos")]
    gpu: gpu::State,
}
impl SurfaceCache {
    pub fn element(&self, height: f32, bubbles: Vec<Bubble>) -> impl IntoElement {
        self.element_with_shelf(height, bubbles, None)
    }
    /// The attachment aperture is a separate closed contour, rather than a
    /// notch connected to the upper edge. The existing member silhouette is
    /// retained outside the attachment's local patch.
    pub fn element_with_attachments(
        &self,
        height: f32,
        bubbles: Vec<Bubble>,
        opening: super::attachment_fan::Opening,
    ) -> impl IntoElement {
        let cache = self.0.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let height = height + TOP_EXTENSION;
                let plate = Bounds::new(
                    point(bounds.left(), bounds.bottom() - px(height)),
                    size(bounds.size.width, px(height)),
                );
                let mut cache = cache.borrow_mut();
                let key = (plate, height, bubbles.clone(), None);
                if cache.key.as_ref() != Some(&key) {
                    (cache.path, cache.border) = silhouette(plate, &bubbles, None);
                    cache.key = Some(key);
                }
                if cache.attachment_key != Some((plate, opening)) {
                    (
                        cache.attachment_path,
                        cache.attachment_border,
                        cache.attachment_hole_border,
                        cache.attachment_gap,
                    ) = attachment_contour(plate, opening);
                    cache.attachment_key = Some((plate, opening));
                }
                let (left, right) = cache.attachment_gap;
                // Fractional mask boundaries each blend with the window and
                // otherwise leave a faint vertical line where the two meet.
                let scale = window.scale_factor();
                // A stroke straddles its path. The outer masks must include
                // both halves and their antialiasing footprint.
                let paint_bounds = bounds.dilate(px(BORDER_WIDTH * 0.5 + 1. / scale));
                let left = (left * scale).round() / scale;
                let right = (right * scale).round() / scale;
                for (start, end) in [
                    (paint_bounds.left().as_f32(), left),
                    (right, paint_bounds.right().as_f32()),
                ] {
                    window.with_content_mask(
                        Some(ContentMask {
                            bounds: Bounds::new(
                                point(px(start), paint_bounds.top()),
                                size(px((end - start).max(0.)), paint_bounds.size.height),
                            ),
                        }),
                        |window| {
                            if let Some(path) = &cache.path {
                                window.paint_path(path.clone(), rgb(SURFACE_COLOR));
                            }
                            if let Some(path) = &cache.border {
                                window.paint_path(path.clone(), rgb(BORDER_COLOR));
                            }
                        },
                    );
                }
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: Bounds::new(
                            point(px(left), paint_bounds.top()),
                            size(px((right - left).max(0.)), paint_bounds.size.height),
                        ),
                    }),
                    |window| {
                        if let Some(path) = &cache.attachment_path {
                            window.paint_path(path.clone(), rgb(SURFACE_COLOR));
                        }
                        if let Some(path) = &cache.attachment_border {
                            window.paint_path(path.clone(), rgb(BORDER_COLOR));
                        }
                        if let Some(path) = &cache.attachment_hole_border {
                            window.paint_path(path.clone(), rgb(BORDER_COLOR));
                        }
                    },
                );
            },
        )
        .absolute()
        .size_full()
    }
    pub fn element_with_shelf(
        &self,
        height: f32,
        bubbles: Vec<Bubble>,
        shelf: Option<Shelf>,
    ) -> impl IntoElement {
        let cache = self.0.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let height = height + TOP_EXTENSION;
                #[cfg(target_os = "macos")]
                let gpu_fill = shelf.is_none()
                    && cache
                        .borrow_mut()
                        .gpu
                        .paint(bounds, height, &bubbles, window);
                #[cfg(not(target_os = "macos"))]
                let gpu_fill = false;
                let plate = Bounds::new(
                    point(bounds.left(), bounds.bottom() - px(height)),
                    size(bounds.size.width, px(height)),
                );
                let mut cache = cache.borrow_mut();
                let key = (plate, height, bubbles.clone(), shelf);
                if cache.key.as_ref() != Some(&key) {
                    (cache.path, cache.border) = silhouette(plate, &bubbles, shelf);
                    cache.key = Some(key);
                }
                if !gpu_fill {
                    if let Some(path) = &cache.path {
                        window.paint_path(path.clone(), rgb(SURFACE_COLOR));
                    }
                }
                if let Some(border) = &cache.border {
                    window.paint_path(border.clone(), rgb(BORDER_COLOR));
                }
            },
        )
        .absolute()
        .size_full()
    }
}

fn attachment_contour(
    plate: Bounds<Pixels>,
    opening: super::attachment_fan::Opening,
) -> (
    Option<Path<Pixels>>,
    Option<Path<Pixels>>,
    Option<Path<Pixels>>,
    (f32, f32),
) {
    let opening = opening.translated(point(plate.left().as_f32(), plate.top().as_f32()));
    let left = opening.outer[0][0].x;
    let right = opening.outer[1][3].x;
    let mut fill = PathBuilder::fill().with_style(PathStyle::Fill(
        FillOptions::default().with_fill_rule(FillRule::EvenOdd),
    ));
    let mut border = PathBuilder::stroke(px(BORDER_WIDTH));
    let mut hole_border = PathBuilder::stroke(px(SLOT_BORDER_WIDTH));
    // Keep artificial vertical fill edges outside the masks to avoid seams.
    fill.move_to(pt(left - 2., plate.top().as_f32()));
    fill.line_to(pt(left, plate.top().as_f32()));
    border.move_to(pt(left, plate.top().as_f32()));
    for path in [&mut fill, &mut border] {
        for c in opening.outer {
            path.cubic_bezier_to(pt(c[3].x, c[3].y), pt(c[1].x, c[1].y), pt(c[2].x, c[2].y));
        }
    }
    fill.line_to(pt(right + 2., plate.top().as_f32()));
    fill.line_to(pt(right + 2., plate.bottom().as_f32()));
    fill.line_to(pt(left - 2., plate.bottom().as_f32()));
    fill.close();
    border.move_to(pt(left, plate.bottom().as_f32()));
    border.line_to(pt(right, plate.bottom().as_f32()));
    if (opening.hole[1][3].x - opening.hole[5][3].x).abs() > 0.001 {
        for path in [&mut fill, &mut hole_border] {
            path.move_to(pt(opening.hole[0][0].x, opening.hole[0][0].y));
            for c in opening.hole {
                path.cubic_bezier_to(pt(c[3].x, c[3].y), pt(c[1].x, c[1].y), pt(c[2].x, c[2].y));
            }
            path.close();
        }
    }
    (
        fill.build().ok(),
        border.build().ok(),
        hole_border.build().ok(),
        (
            left.max(plate.left().as_f32() + SURFACE_RADIUS),
            right.min(plate.right().as_f32() - SURFACE_RADIUS),
        ),
    )
}

fn pt(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

// All contours wind clockwise, so nonzero fill merges overlaps before antialiasing.
fn rounded(path: &mut PathBuilder, x: f32, y: f32, width: f32, height: f32, r: f32, bottom_r: f32) {
    let k = 0.5522848;
    let right = x + width;
    let bottom = y + height;
    path.move_to(pt(x + r, y));
    path.line_to(pt(right - r, y));
    path.cubic_bezier_to(
        pt(right, y + r),
        pt(right - r + r * k, y),
        pt(right, y + r - r * k),
    );
    path.line_to(pt(right, bottom - bottom_r));
    path.cubic_bezier_to(
        pt(right - bottom_r, bottom),
        pt(right, bottom - bottom_r + bottom_r * k),
        pt(right - bottom_r + bottom_r * k, bottom),
    );
    path.line_to(pt(x + bottom_r, bottom));
    path.cubic_bezier_to(
        pt(x, bottom - bottom_r),
        pt(x + bottom_r - bottom_r * k, bottom),
        pt(x, bottom - bottom_r + bottom_r * k),
    );
    path.line_to(pt(x, y + r));
    path.cubic_bezier_to(pt(x + r, y), pt(x, y + r - r * k), pt(x + r - r * k, y));
    path.close();
}

fn silhouette(
    plate: Bounds<Pixels>,
    bubbles: &[Bubble],
    shelf: Option<Shelf>,
) -> (Option<Path<Pixels>>, Option<Path<Pixels>>) {
    let x = plate.left().as_f32();
    let y = plate.top().as_f32();
    let width = plate.size.width.as_f32();
    let height = plate.size.height.as_f32();
    let mut paths = [
        PathBuilder::fill().with_style(PathStyle::Fill(
            FillOptions::default().with_fill_rule(FillRule::NonZero),
        )),
        PathBuilder::stroke(px(BORDER_WIDTH)),
    ];
    if bubbles.is_empty() && shelf.is_none() {
        for path in &mut paths {
            rounded(path, x, y, width, height, SURFACE_RADIUS, SURFACE_RADIUS);
        }
    } else {
        append_liquid_contours(&mut paths, x, y, width, height, bubbles, shelf);
    }
    let [fill, border] = paths;
    (fill.build().ok(), border.build().ok())
}

// Trace one complete silhouette for both fill and an equal-width geometric
// stroke. Artificial cuts inside the plate must never become visible borders.
fn append_liquid_contours(
    paths: &mut [PathBuilder; 2],
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    bubbles: &[Bubble],
    shelf: Option<Shelf>,
) {
    const STEP: f32 = 1.;
    #[cfg(feature = "headless-bench")]
    let blur = shelf.is_none() && std::env::var_os("ZORK_LIQUID_COMPARE_BLUR").is_some();
    #[cfg(not(feature = "headless-bench"))]
    let blur = false;
    const PAD: f32 = 36.;
    const THRESHOLD: f32 = 0.5;
    let left = -PAD;
    let top = bubbles
        .iter()
        .map(|b| -RADIUS - b.lift)
        .fold(0., f32::min)
        .min(-shelf.map_or(0., |s| s.height))
        - PAD;
    let right = width + PAD;
    let cols = ((right - left) / STEP).ceil() as usize + 1;
    let rows = ((height + PAD - top) / STEP).ceil() as usize + 1;
    if !blur {
        let field = |x, y| {
            let mut d = field::scalar(x, y, width, bubbles);
            if let Some(shelf) = shelf {
                d = d.min(field::scalar(x, y + shelf.rise(x, width), width, &[]));
            }
            let d = d.max(field::scalar(x, height - y, width, &[]));
            0.5 - d / STEP
        };
        #[cfg(target_arch = "aarch64")]
        let simd = shelf.is_none()
            && std::env::var_os("ZORK_LIQUID_SIMD").is_some()
            && std::env::var_os("ZORK_LIQUID_SCALAR").is_none();
        let mut mask = vec![f32::NAN; cols * rows];
        let sample = |index: usize, mask: &mut [f32]| {
            if mask[index].is_nan() {
                let row = index / cols;
                let col = index % cols;
                mask[index] = if row == 0 || row == rows - 1 || col == 0 || col == cols - 1 {
                    0.
                } else {
                    #[cfg(target_arch = "aarch64")]
                    if simd {
                        let r = row & !1;
                        let c = col & !1;
                        let rr = (r + 1).min(rows - 1);
                        let cc = (c + 1).min(cols - 1);
                        let xs = [
                            left + c as f32 * STEP,
                            left + cc as f32 * STEP,
                            left + c as f32 * STEP,
                            left + cc as f32 * STEP,
                        ];
                        let ys = [
                            top + r as f32 * STEP,
                            top + r as f32 * STEP,
                            top + rr as f32 * STEP,
                            top + rr as f32 * STEP,
                        ];
                        let values = field::four(xs, ys, width, bubbles);
                        for (lane, (y, x)) in
                            [(r, c), (r, cc), (rr, c), (rr, cc)].into_iter().enumerate()
                        {
                            mask[y * cols + x] =
                                if y == 0 || y == rows - 1 || x == 0 || x == cols - 1 {
                                    0.
                                } else {
                                    0.5 - values[lane].max(field::scalar(
                                        xs[lane],
                                        height - ys[lane],
                                        width,
                                        &[],
                                    )) / STEP
                                };
                        }
                        return mask[index];
                    }
                    field(left + col as f32 * STEP, top + row as f32 * STEP)
                };
            }
            mask[index]
        };
        let mut queue = std::collections::VecDeque::new();
        // Every component contains a source shape. Its centre column supplies
        // a boundary seed; the flood then visits only contour-adjacent cells.
        for sx in std::iter::once(width * 0.5)
            .chain(bubbles.iter().map(|b| b.x + b.width * 0.5))
            .chain(shelf.map(|s| width - s.inset - s.width * 0.5))
        {
            let col = (((sx - left) / STEP) as usize).min(cols - 2);
            for row in 0..rows - 1 {
                let i = row * cols + col;
                if (sample(i, &mut mask) >= THRESHOLD) != (sample(i + cols, &mut mask) >= THRESHOLD)
                {
                    queue.push_back(i);
                }
            }
        }
        let mut visited = vec![false; cols * rows];
        let mut cells = Vec::new();
        while let Some(i) = queue.pop_front() {
            if visited[i] {
                continue;
            }
            visited[i] = true;
            let row = i / cols;
            let col = i % cols;
            let values = [
                sample(i, &mut mask),
                sample(i + 1, &mut mask),
                sample(i + cols + 1, &mut mask),
                sample(i + cols, &mut mask),
            ];
            let mut crossing = false;
            for edge in 0..4 {
                if (values[edge] >= THRESHOLD) == (values[(edge + 1) % 4] >= THRESHOLD) {
                    continue;
                }
                crossing = true;
                match edge {
                    0 if row > 0 => queue.push_back(i - cols),
                    1 if col < cols - 2 => queue.push_back(i + 1),
                    2 if row < rows - 2 => queue.push_back(i + cols),
                    3 if col > 0 => queue.push_back(i - 1),
                    _ => {}
                }
            }
            if crossing {
                cells.push(i);
            }
        }
        append_isolines(
            paths,
            &mask,
            cols,
            rows,
            THRESHOLD,
            pt(x + left, y + top),
            STEP,
            &cells,
        );
        return;
    }
    let influence_right = bubbles
        .iter()
        .map(|b| b.x + b.width + 36.)
        .fold(0., f32::max);
    let mut mask = vec![0.; cols * rows];
    for row in 0..rows {
        let py = top + row as f32 * STEP;
        for col in 0..cols {
            let px = left + col as f32 * STEP;
            let corner_x = px.clamp(SURFACE_RADIUS, (width - SURFACE_RADIUS).max(SURFACE_RADIUS));
            let corner_y = py.max(SURFACE_RADIUS);
            let plate_distance = if px >= SURFACE_RADIUS && px <= width - SURFACE_RADIUS {
                -py.min(SURFACE_RADIUS)
            } else {
                ((px - corner_x).powi(2) + (py - corner_y).powi(2)).sqrt() - SURFACE_RADIUS
            };
            let mut distance = plate_distance;
            for b in bubbles.iter().filter(|_| px <= influence_right) {
                let start = b.x + 20.;
                let center_x = px.clamp(start, (b.x + b.width - 20.).max(start));
                let d = ((px - center_x).powi(2) + (py + b.lift).powi(2)).sqrt()
                    - if blur { 22. } else { 20. };
                if blur {
                    distance = distance.min(d);
                } else {
                    // Compact smooth union: no offscreen image or blur passes.
                    let k = 36.;
                    let h = ((k - (distance - d).abs()) / k).max(0.);
                    distance = distance.min(d) - h * h * k * 0.25;
                }
            }
            distance = distance.max(field::scalar(px, height - py, width, &[]));
            mask[row * cols + col] = if blur {
                (0.5 - distance / STEP).clamp(0., 1.)
            } else {
                0.5 - distance / STEP
            };
        }
    }
    if blur {
        let mut scratch = vec![0.; mask.len()];
        for _ in 0..3 {
            box_blur(&mask, &mut scratch, cols, rows, true);
            box_blur(&scratch, &mut mask, cols, rows, false);
        }
    }
    // Close the local mask inside the plate. The straight cut is fully buried
    // in the main surface, away from the visible liquid boundary.
    for row in 0..rows {
        mask[row * cols] = 0.;
        mask[row * cols + cols - 1] = 0.;
    }
    mask[..cols].fill(0.);
    mask[(rows - 1) * cols..].fill(0.);
    append_isolines(
        paths,
        &mask,
        cols,
        rows,
        THRESHOLD,
        pt(x + left, y + top),
        STEP,
        &(0..rows - 1)
            .flat_map(|row| (0..cols - 1).map(move |col| row * cols + col))
            .collect::<Vec<_>>(),
    );
}

fn box_blur(source: &[f32], output: &mut [f32], cols: usize, rows: usize, horizontal: bool) {
    const RADIUS: isize = 3;
    let (lines, length, stride, line_stride) = if horizontal {
        (rows, cols, 1, cols)
    } else {
        (cols, rows, cols, 1)
    };
    for line in 0..lines {
        let base = line * line_stride;
        let sample = |at: isize| source[base + at.clamp(0, length as isize - 1) as usize * stride];
        let mut sum: f32 = (-RADIUS..=RADIUS).map(sample).sum();
        for at in 0..length {
            output[base + at * stride] = sum / (2 * RADIUS + 1) as f32;
            sum += sample(at as isize + RADIUS + 1) - sample(at as isize - RADIUS);
        }
    }
}

// Marching squares uses shared edge identities so every threshold boundary is
// a closed contour, including the two loops immediately after a neck breaks.
fn append_isolines(
    paths: &mut [PathBuilder; 2],
    mask: &[f32],
    cols: usize,
    rows: usize,
    threshold: f32,
    origin: Point<Pixels>,
    step: f32,
    cells: &[usize],
) {
    // Grid edges only hold compact indices. Geometry and adjacency are stored
    // for boundary vertices, not for the entire area enclosed by the surface.
    let mut indices = vec![usize::MAX; cols * rows * 2];
    let mut points = Vec::with_capacity(cells.len());
    let mut edges = Vec::with_capacity(cells.len());
    fn connect(edges: &mut [[usize; 2]], a: usize, b: usize) {
        for (from, to) in [(a, b), (b, a)] {
            let slot = if edges[from][0] == usize::MAX { 0 } else { 1 };
            edges[from][slot] = to;
        }
    }
    for &i in cells {
        let row = i / cols;
        let col = i % cols;
        let values = [mask[i], mask[i + 1], mask[i + cols + 1], mask[i + cols]];
        let case = (values[0] >= threshold) as u8
            | (((values[1] >= threshold) as u8) << 1)
            | (((values[2] >= threshold) as u8) << 2)
            | (((values[3] >= threshold) as u8) << 3);
        if case == 0 || case == 15 {
            continue;
        }
        let ids = [i * 2, (i + 1) * 2 + 1, (i + cols) * 2, i * 2 + 1];
        let positions = [
            (col as f32, row as f32),
            ((col + 1) as f32, row as f32),
            ((col + 1) as f32, (row + 1) as f32),
            (col as f32, (row + 1) as f32),
        ];
        let mut crossings = [0; 4];
        let mut crossing_count = 0;
        for edge in 0..4 {
            let next = (edge + 1) % 4;
            if (values[edge] >= threshold) == (values[next] >= threshold) {
                continue;
            }
            let t = (threshold - values[edge]) / (values[next] - values[edge]);
            let (ax, ay) = positions[edge];
            let (bx, by) = positions[next];
            if indices[ids[edge]] == usize::MAX {
                indices[ids[edge]] = points.len();
                points.push(
                    origin
                        + point(
                            px((ax + (bx - ax) * t) * step),
                            px((ay + (by - ay) * t) * step),
                        ),
                );
                edges.push([usize::MAX; 2]);
            }
            crossings[crossing_count] = indices[ids[edge]];
            crossing_count += 1;
        }
        if crossing_count == 2 {
            connect(&mut edges, crossings[0], crossings[1]);
        } else if crossing_count == 4 {
            if (values.iter().sum::<f32>() * 0.25 >= threshold) == (values[0] >= threshold) {
                connect(&mut edges, crossings[0], crossings[1]);
                connect(&mut edges, crossings[2], crossings[3]);
            } else {
                connect(&mut edges, crossings[0], crossings[3]);
                connect(&mut edges, crossings[1], crossings[2]);
            }
        }
    }
    let mut visited = vec![false; points.len()];
    for start in 0..points.len() {
        if visited[start] {
            continue;
        }
        let mut contour = Vec::new();
        let mut previous = usize::MAX;
        let mut current = start;
        while !visited[current] {
            visited[current] = true;
            contour.push(points[current]);
            let next = if edges[current][0] != previous {
                edges[current][0]
            } else {
                edges[current][1]
            };
            if next == usize::MAX {
                break;
            }
            previous = current;
            current = next;
        }
        if contour.len() < 3 {
            continue;
        }
        let area: f32 = contour
            .iter()
            .zip(contour.iter().cycle().skip(1))
            .map(|(a, b)| a.x.as_f32() * b.y.as_f32() - b.x.as_f32() * a.y.as_f32())
            .sum();
        if area < 0. {
            contour.reverse();
        }
        let half = contour.len() / 2;
        let mut reduced = Vec::new();
        simplify(&contour[..=half], &mut reduced);
        let mut rest = contour[half..].to_vec();
        rest.push(contour[0]);
        simplify(&rest, &mut reduced);
        contour = reduced;
        for path in paths.iter_mut() {
            path.move_to(contour[0]);
        }
        let n = contour.len();
        let length = |v: Point<Pixels>| {
            (v.x.as_f32().powi(2) + v.y.as_f32().powi(2))
                .sqrt()
                .max(0.0001)
        };
        let tangent = |before: Point<Pixels>, at: Point<Pixels>, after: Point<Pixels>| {
            let a = at - before;
            let b = after - at;
            let sum = a / length(a) + b / length(b);
            sum / length(sum)
        };
        for i in 0..n {
            let previous = contour[(i + n - 1) % n];
            let a = contour[i];
            let b = contour[(i + 1) % n];
            let after = contour[(i + 2) % n];
            let distance = length(b - a) / 3.;
            let ca = a + tangent(previous, a, b) * distance.min(length(a - previous) / 3.);
            let cb = b - tangent(a, b, after) * distance.min(length(after - b) / 3.);
            for path in paths.iter_mut() {
                path.cubic_bezier_to(b, ca, cb);
            }
        }
        for path in paths.iter_mut() {
            path.close();
        }
    }
}

// Ramer–Douglas–Peucker removes redundant samples with a subpixel error bound.
// Long straight plate edges otherwise dominate tessellation for no visual gain.
fn simplify(points: &[Point<Pixels>], output: &mut Vec<Point<Pixels>>) {
    if points.len() < 2 {
        return;
    }
    let a = points[0];
    let b = points[points.len() - 1];
    let dx = (b.x - a.x).as_f32();
    let dy = (b.y - a.y).as_f32();
    let norm = (dx * dx + dy * dy).sqrt().max(0.0001);
    let (index, distance) = points
        .iter()
        .enumerate()
        .skip(1)
        .take(points.len() - 2)
        .map(|(i, p)| {
            (
                i,
                ((p.x - a.x).as_f32() * dy - (p.y - a.y).as_f32() * dx).abs() / norm,
            )
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0, 0.));
    if distance > 0.12 {
        simplify(&points[..=index], output);
        simplify(&points[index..], output);
    } else {
        output.push(a);
    }
}

fn smooth_union(a: f32, b: f32, k: f32) -> f32 {
    let h = ((k - (a - b).abs()) / k).max(0.);
    a.min(b) - h * h * k * 0.25
}
