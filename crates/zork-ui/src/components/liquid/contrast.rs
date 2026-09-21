//! Two inks over one fixed text layout. Device-pixel spans partition the live
//! contour, including holes; neither selected indices nor target poses clip ink.
use super::*;

impl ContentClip {
    pub fn contrast_label(
        &self,
        text: SharedString,
        outside: u32,
        inside: u32,
    ) -> impl IntoElement {
        let clip = self.clone();
        canvas(
            move |_, window, _| {
                let style = window.text_style();
                let font_size = style.font_size.to_pixels(window.rem_size());
                let line_height = window.pixel_snap(
                    style
                        .line_height
                        .to_pixels(font_size.into(), window.rem_size()),
                );
                let lines = [outside, inside].map(|color| {
                    let mut run = style.to_run(text.len());
                    run.color = rgb(color).into();
                    window
                        .text_system()
                        .shape_line(text.clone(), font_size, &[run], None)
                });
                (lines, line_height)
            },
            move |bounds, (lines, line_height), window, cx| {
                let origin = bounds.origin
                    + point(
                        (bounds.size.width - lines[0].width()) / 2.,
                        (bounds.size.height - line_height) / 2.,
                    );
                // Retain glyph overhangs and focus underlines. The layout's
                // font fallback, baseline and raster atlas are shared by both inks.
                let ink = Bounds::new(
                    origin - point(px(2.), px(2.)),
                    size(lines[0].width() + px(4.), line_height + px(4.)),
                )
                .intersect(&window.content_mask().bounds);
                if ink.size.width <= px(0.) || ink.size.height <= px(0.) {
                    return;
                }
                let surface_origin = clip.paint.0.borrow().bounds.origin;
                let scale = window.scale_factor() as f64;
                let region = [
                    (ink.left().as_f32() as f64 * scale).floor() as i32,
                    (ink.top().as_f32() as f64 * scale).floor() as i32,
                    (ink.right().as_f32() as f64 * scale).ceil() as i32,
                    (ink.bottom().as_f32() as f64 * scale).ceil() as i32,
                ];
                let spans = partition(
                    &clip.polygons,
                    [
                        surface_origin.x.as_f32() as f64,
                        surface_origin.y.as_f32() as f64,
                    ],
                    region,
                    scale,
                );
                for span in spans {
                    // GPUI covers a scissor's fractional bounds outwards. Inset
                    // by a tiny fraction of a device pixel to prevent floating
                    // point roundoff from duplicating adjacent rows/columns.
                    let [left, top, right, bottom] = span.rect;
                    let logical = |v: f64| px((v / scale) as f32);
                    let mask = ContentMask {
                        bounds: Bounds::from_corners(
                            point(logical(left as f64 + 0.001), logical(top as f64 + 0.001)),
                            point(
                                logical(right as f64 - 0.001),
                                logical(bottom as f64 - 0.001),
                            ),
                        ),
                    };
                    window.with_content_mask(Some(mask), |window| {
                        let _ = lines[span.inside as usize].paint(
                            origin,
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    });
                }
            },
        )
        .size_full()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InkSpan {
    rect: [i32; 4],
    inside: bool,
}

/// Scan the same flattened curves as material feedback at device-pixel centers.
/// Adjacent equal runs coalesce, so a settled label is painted just once. The
/// complementary inks never overlap, preserving glyph antialiasing and weight.
fn partition(polygons: &Polygons, origin: Point, region: [i32; 4], scale: f64) -> Vec<InkSpan> {
    let [left, top, right, bottom] = region;
    let mut spans: Vec<InkSpan> = Vec::new();
    let mut previous: Vec<usize> = Vec::new();
    let mut current = Vec::new();
    let mut xs = Vec::new();
    for row in top..bottom {
        let y = (row as f64 + 0.5) / scale - origin[1];
        xs.clear();
        for polygon in polygons {
            for (&a, &b) in polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .take(polygon.len())
            {
                if (a[1] <= y && b[1] > y) || (b[1] <= y && a[1] > y) {
                    xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        current.clear();
        let mut append = |x1, x2, inside| {
            if x2 <= x1 {
                return;
            }
            if let Some(&index) = previous.iter().find(|&&i| {
                let span = spans[i];
                span.rect[0] == x1 && span.rect[2] == x2 && span.inside == inside
            }) {
                spans[index].rect[3] = row + 1;
                current.push(index);
            } else {
                current.push(spans.len());
                spans.push(InkSpan {
                    rect: [x1, row, x2, row + 1],
                    inside,
                });
            }
        };
        let mut cursor = left;
        for pair in xs.chunks_exact(2) {
            let x1 = ((pair[0] + origin[0]) * scale).round() as i32;
            let x2 = ((pair[1] + origin[0]) * scale).round() as i32;
            let start = x1.clamp(cursor, right);
            let end = x2.clamp(start, right);
            if start < end {
                append(cursor, start, false);
                append(start, end, true);
                cursor = end;
            }
        }
        append(cursor, right, false);
        std::mem::swap(&mut previous, &mut current);
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::liquid::{Material, Options};

    #[core::prelude::v1::test]
    fn ink_partition_matches_live_contour_without_overlap_or_gaps() {
        let mut simulation = Simulation::new(
            Pose::rect(4., 4., 94., 24., 12.),
            Material::ordinary(),
            Options::default(),
        );
        simulation.finish();
        simulation.set_target(Pose::rect(103., 4., 94., 24., 12.));
        for frame in 0..26 {
            if frame == 8 {
                simulation.set_target(Pose::rect(4., 4., 94., 24., 12.));
            }
            simulation.advance(1. / 120., false);
            let surface = Surface::new(simulation.clone()).unwrap();
            let contour = surface.contour();
            let polygons = contour.loops.iter().map(|curves| flatten(curves)).collect();
            for scale in [1., 1.5, 2., 3.] {
                let origin = [17.3, 9.7];
                let region = [0, 0, (225. * scale) as i32, (52. * scale) as i32];
                let spans = partition(&polygons, origin, region, scale);
                let width = region[2] as usize;
                let mut pixels = vec![(0_u8, false); width * region[3] as usize];
                for span in spans {
                    for y in span.rect[1]..span.rect[3] {
                        for x in span.rect[0]..span.rect[2] {
                            let pixel = &mut pixels[y as usize * width + x as usize];
                            pixel.0 += 1;
                            pixel.1 = span.inside;
                        }
                    }
                }
                for y in 0..region[3] {
                    for x in 0..region[2] {
                        let (count, inside) = pixels[y as usize * width + x as usize];
                        assert_eq!(count, 1, "every pixel has exactly one ink");
                        let local = [
                            (x as f64 + 0.5) / scale - origin[0],
                            (y as f64 + 0.5) / scale - origin[1],
                        ];
                        if contour.contains(local) != inside {
                            // Flattening uses the renderer's 0.01px tolerance;
                            // classification can differ only at that boundary.
                            assert!([[-0.02, 0.], [0.02, 0.], [0., -0.02], [0., 0.02]]
                                .iter()
                                .any(|d| contour.contains([local[0] + d[0], local[1] + d[1]])
                                    == inside));
                        }
                    }
                }
            }
        }
    }

    #[core::prelude::v1::test]
    fn ink_partition_keeps_holes_and_coalesces_settled_regions() {
        let outer = vec![[0., 0.], [30., 0.], [30., 30.], [0., 30.]];
        let hole = vec![[10., 10.], [20., 10.], [20., 20.], [10., 20.]];
        let spans = partition(&vec![outer.clone(), hole], [0., 0.], [0, 0, 30, 30], 1.);
        assert_eq!(
            spans
                .iter()
                .filter(|s| !s.inside)
                .map(|s| s.rect)
                .collect::<Vec<_>>(),
            vec![[10, 10, 20, 20]]
        );
        assert_eq!(
            partition(&vec![outer], [0., 0.], [2, 2, 28, 28], 1.),
            vec![InkSpan {
                rect: [2, 2, 28, 28],
                inside: true
            }]
        );
    }
}
