//! Keep hairlines inside the material's outer contour, including holes.
use super::Contour;
#[cfg(test)]
use super::Cubic;
use gpui::{FillOptions, FillRule, Path, PathBuilder, PathStyle, Pixels};
// A subpixel border must not inherit the tessellator's coarse default.
pub(crate) use zork_liquid::border::TOLERANCE;

pub(crate) fn fill_builder() -> PathBuilder {
    PathBuilder::fill().with_style(PathStyle::Fill(
        FillOptions::default()
            .with_fill_rule(FillRule::EvenOdd)
            .with_tolerance(TOLERANCE),
    ))
}

pub(crate) fn fill_path(contour: &Contour) -> Option<Path<Pixels>> {
    let mut fill = fill_builder();
    super::render::append(&mut fill, contour);
    fill.build().ok()
}

pub(crate) fn stroke_path(contour: &Contour, width: f32) -> Option<Path<Pixels>> {
    Some(PathBuilder::build_path(zork_liquid::border::mesh(contour, width)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::liquid::{rounded_rectangle, Pose};
    use gpui::px;

    fn contour(loops: Vec<Vec<Cubic>>) -> Contour {
        Contour {
            loops,
            sampled_points: 0,
            full_grid_points: 0,
            used_fallback: false,
            revision: 0,
        }
    }

    #[test]
    fn hairline_stays_inside_the_outer_bounds() {
        for (w, h, radius) in [
            (76., 32., 16.),
            (96., 32., 12.),
            (2., 2., 1.),
            (540., 320., 32.),
        ] {
            let shape = contour(vec![rounded_rectangle(
                Pose::rect(0., 0., w, h, radius),
                0.6,
            )]);
            let border = stroke_path(&shape, 0.5).unwrap();
            assert!(!border.vertices.is_empty());
            for vertex in border.vertices.iter() {
                let p = vertex.xy_position;
                assert!(p.x >= px(-0.0001) && p.x <= px(w as f32 + 0.0001), "{p:?}");
                assert!(p.y >= px(-0.0001) && p.y <= px(h as f32 + 0.0001), "{p:?}");
            }
        }
    }

    #[test]
    fn holes_and_reversed_loops_stroke_toward_the_material() {
        let outer = rounded_rectangle(Pose::rect(0., 0., 100., 80., 12.), 0.6);
        let hole = rounded_rectangle(Pose::rect(24., 24., 32., 24., 6.), 0.6);
        let shape = contour(vec![outer.clone(), hole.clone()]);
        for loops in [
            vec![outer, hole],
            shape
                .loops
                .iter()
                .map(|curves| {
                    curves
                        .iter()
                        .rev()
                        .map(|c| Cubic {
                            from: c.to,
                            c1: c.c2,
                            c2: c.c1,
                            to: c.from,
                        })
                        .collect()
                })
                .collect(),
        ] {
            let border = stroke_path(&contour(loops), 0.5).unwrap();
            for triangle in border.vertices.chunks_exact(3) {
                let a = triangle[1].xy_position - triangle[0].xy_position;
                let b = triangle[2].xy_position - triangle[0].xy_position;
                if (a.x.as_f32() * b.y.as_f32() - a.y.as_f32() * b.x.as_f32()).abs() < 1e-6 {
                    continue;
                }
                let midpoint = std::array::from_fn(|axis| {
                    triangle
                        .iter()
                        .map(|v| {
                            if axis == 0 {
                                v.xy_position.x.as_f32() as f64
                            } else {
                                v.xy_position.y.as_f32() as f64
                            }
                        })
                        .sum::<f64>()
                        / 3.
                });
                assert!(
                    shape.contains(midpoint),
                    "border entered a hole or left the material: {midpoint:?}"
                );
            }
        }
    }
}
