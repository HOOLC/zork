//! Inward border geometry shared by all drawing backends.
use crate::{Contour, Cubic};
use lyon::{
    math::point,
    path::Side,
    tessellation::{BuffersBuilder, StrokeOptions, StrokeTessellator, StrokeVertex, VertexBuffers},
};
pub const TOLERANCE: f32 = 0.01;

/// Boundary of the exact inward mesh. Canvas backends can fill these loops
/// with antialiasing without rebuilding a path for every interior triangle.
pub fn outline(contour: &Contour, width: f32) -> Option<Vec<Vec<[f32; 2]>>> {
    use rustc_hash::FxHashMap;
    let mesh = mesh(contour, width)?;
    let mut ids = FxHashMap::default();
    let mut points = Vec::new();
    let mut edges: FxHashMap<(usize, usize), usize> = FxHashMap::default();
    for triangle in mesh.indices.chunks_exact(3) {
        let p = [triangle[0], triangle[1], triangle[2]].map(|i| mesh.vertices[i as usize]);
        if ((p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[1].y - p[0].y) * (p[2].x - p[0].x)).abs()
            < 1e-10
        {
            continue;
        }
        let vertex: [usize; 3] = std::array::from_fn(|i| {
            let point = [p[i].x, p[i].y].map(|v| if v == 0. { 0. } else { v });
            *ids.entry(point.map(f32::to_bits)).or_insert_with(|| {
                points.push(point);
                points.len() - 1
            })
        });
        for (a, b) in [
            (vertex[0], vertex[1]),
            (vertex[1], vertex[2]),
            (vertex[2], vertex[0]),
        ] {
            if a == b {
                continue;
            }
            if let Some(count) = edges.get_mut(&(b, a)) {
                *count -= 1;
                if *count == 0 {
                    edges.remove(&(b, a));
                }
            } else {
                *edges.entry((a, b)).or_default() += 1;
            }
        }
    }
    let mut ordered: Vec<_> = edges.into_iter().collect();
    ordered.sort_unstable();
    let edge_count: usize = ordered.iter().map(|(_, count)| count).sum();
    // One flat adjacency list avoids a heap allocation for every boundary
    // vertex (thousands per animated frame at subpixel border tolerance).
    let mut heads = vec![usize::MAX; points.len()];
    let mut links = Vec::with_capacity(edge_count);
    for ((a, b), count) in ordered {
        for _ in 0..count {
            links.push((b, heads[a]));
            heads[a] = links.len() - 1;
        }
    }
    let mut output = Vec::new();
    for start in 0..points.len() {
        while heads[start] != usize::MAX {
            let mut path = vec![points[start]];
            let mut current = start;
            loop {
                let (next, rest) = *links.get(heads[current])?;
                heads[current] = rest;
                current = next;
                if current == start {
                    break;
                }
                path.push(points[current]);
                if path.len() > edge_count + 1 {
                    return None;
                }
            }
            if path.len() >= 3 {
                output.push(path);
            }
        }
    }
    Some(output)
}

fn interior_side(contour: &Contour, curves: &[Cubic]) -> Option<Side> {
    curves.iter().find_map(|curve| {
        let tangent: [f64; 2] = std::array::from_fn(|i| {
            0.75 * (curve.c1[i] - curve.from[i])
                + 1.5 * (curve.c2[i] - curve.c1[i])
                + 0.75 * (curve.to[i] - curve.c2[i])
        });
        let length = tangent[0].hypot(tangent[1]);
        if length < 1e-8 {
            return None;
        }
        let midpoint = curve.at(0.5);
        let positive = [
            midpoint[0] - tangent[1] / length * 0.01,
            midpoint[1] + tangent[0] / length * 0.01,
        ];
        Some(if contour.contains(positive) {
            Side::Positive
        } else {
            Side::Negative
        })
    })
}

pub fn mesh(contour: &Contour, width: f32) -> Option<VertexBuffers<lyon::math::Point, u16>> {
    if !width.is_finite() || width <= 0. {
        return None;
    }
    let mut vertices: VertexBuffers<lyon::math::Point, u16> = VertexBuffers::new();
    let mut tessellator = StrokeTessellator::new();
    let options = StrokeOptions::default()
        .with_line_width(2. * width)
        .with_tolerance(TOLERANCE);
    for curves in &contour.loops {
        let Some(inside) = interior_side(contour, curves) else {
            continue;
        };
        let mut path = lyon::path::Path::builder().with_svg();
        let p = |p: [f64; 2]| point(p[0] as f32, p[1] as f32);
        path.move_to(p(curves[0].from));
        for curve in curves {
            path.cubic_bezier_to(p(curve.c1), p(curve.c2), p(curve.to));
        }
        path.close();
        tessellator
            .tessellate_path(
                &path.build(),
                &options,
                &mut BuffersBuilder::new(&mut vertices, |vertex: StrokeVertex| {
                    // Lyon owns joins and curve subdivision. Collapse only the
                    // outside half to the original boundary; keep the full
                    // requested width on the material side of every loop.
                    if vertex.side() == inside {
                        vertex.position()
                    } else {
                        vertex.position_on_path()
                    }
                }),
            )
            .ok()?;
    }
    Some(vertices)
}
