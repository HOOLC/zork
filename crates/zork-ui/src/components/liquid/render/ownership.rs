//! One contour, two paint owners. Partition existing fill and border triangles
//! together: the ownership boundary never introduces an internal stroke.
use super::*;

pub(super) use zork_liquid::ownership::MaterialPartition as HalfPlane;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Partition {
    Plane(HalfPlane),
    Body {
        pose: Pose,
        plane: HalfPlane,
        inside: bool,
    },
}
impl From<HalfPlane> for Partition {
    fn from(plane: HalfPlane) -> Self {
        Self::Plane(plane)
    }
}

#[derive(Clone)]
pub(crate) struct MaterialPart {
    path: Rc<Contour>,
    paint: PaintHandle,
}
impl MaterialPart {
    pub(crate) fn paint_geometry(
        &self,
        bounds: Bounds<Pixels>,
        fill: Option<u32>,
        stroke: Option<u32>,
        window: &mut Window,
    ) {
        let mut cache = self.paint.0.borrow_mut();
        cache.prepare(&self.path, bounds);
        if let (Some(path), Some(color)) = (&cache.fill, fill) {
            paint_at(window, path, bounds.origin, color);
        }
        if let (Some(path), Some(color)) = (&cache.stroke, stroke) {
            paint_at(window, path, bounds.origin, color);
        }
    }

    pub(crate) fn background(
        &self,
        fill: Option<u32>,
        stroke: Option<u32>,
        offset: Point<Pixels>,
        focused: bool,
    ) -> AnyElement {
        background(
            self.path.clone(),
            self.paint.clone(),
            fill,
            stroke,
            offset,
            focused,
        )
        .into_any_element()
    }
    pub(crate) fn content_clip(&self) -> ContentClip {
        ContentClip::new(self.path.clone(), self.paint.clone())
    }
}

#[derive(Clone)]
pub(crate) struct SourceFrame {
    pub part: MaterialPart,
    pub rest: Pose,
    pub pose: Pose,
    owner: Option<SharedString>,
}

/// A component-local presentation binding. The original control resolves its
/// surface in prepaint, after the panel has advanced the shared material.
#[derive(Clone, Default)]
pub struct SourceMaterial {
    frame: Rc<RefCell<Option<SourceFrame>>>,
    pub(super) owner: Option<SharedString>,
    pub(super) presentation: Rc<RefCell<super::presentation::Presentation>>,
}
impl SourceMaterial {
    pub(crate) fn for_owner(&self, owner: SharedString) -> Self {
        Self {
            frame: self.frame.clone(),
            owner: Some(owner),
            presentation: self.presentation.clone(),
        }
    }
    pub(crate) fn clear(&self) {
        self.frame.borrow_mut().take();
    }
    pub(crate) fn resolve(&self) -> Option<SourceFrame> {
        if self.presentation.borrow().enabled {
            return None;
        }
        self.frame
            .borrow()
            .as_ref()
            .filter(|f| self.owner.is_none() || self.owner == f.owner)
            .cloned()
    }
    pub(crate) fn bind(
        &self,
        surface: &Surface,
        rest: Pose,
        owner: Option<SharedString>,
    ) -> MaterialPart {
        let source = surface.simulation.source_pose();
        let plane = surface.simulation.source_partition();
        let body = surface.simulation.pose();
        let ownership = |inside: bool| {
            if HalfPlane::crossing(source, body) {
                Partition::Body {
                    pose: body,
                    plane: plane.reversed(),
                    inside: !inside,
                }
            } else {
                Partition::Plane(if inside { plane } else { plane.reversed() })
            }
        };
        let make = |index: usize, plane| {
            let paint = surface.owners[index].clone();
            let mut cache = paint.0.borrow_mut();
            if cache.partition != Some(plane) {
                cache.partition = Some(plane);
                cache.key = None;
            }
            drop(cache);
            MaterialPart {
                path: surface.contour(),
                paint,
            }
        };
        *self.frame.borrow_mut() = Some(SourceFrame {
            part: make(0, ownership(true)),
            rest,
            pose: source,
            owner,
        });
        make(1, ownership(false))
    }
}

fn cut(vertices: &[PathVertex<Pixels>], plane: HalfPlane) -> Vec<PathVertex<Pixels>> {
    let mut polygon = Vec::with_capacity(vertices.len() + 1);
    for i in 0..vertices.len() {
        let a = &vertices[i];
        let b = &vertices[(i + 1) % vertices.len()];
        let xy = |v: &PathVertex<Pixels>| {
            [
                v.xy_position.x.as_f32() as f64,
                v.xy_position.y.as_f32() as f64,
            ]
        };
        let da = plane.distance(xy(a));
        let db = plane.distance(xy(b));
        if da <= 0. {
            polygon.push(a.clone());
        }
        if (da <= 0.) != (db <= 0.) {
            let t = (da / (da - db)) as f32;
            let mut vertex = a.clone();
            vertex.xy_position = a.xy_position + (b.xy_position - a.xy_position) * t;
            vertex.st_position = a.st_position + (b.st_position - a.st_position) * t;
            polygon.push(vertex);
        }
    }
    polygon
}
fn append_polygon(path: &mut Path<Pixels>, polygon: &[PathVertex<Pixels>]) {
    for i in 1..polygon.len().saturating_sub(1) {
        path.vertices_mut().extend([
            polygon[0].clone(),
            polygon[i].clone(),
            polygon[i + 1].clone(),
        ]);
    }
}
pub(super) fn partition(mut path: Path<Pixels>, region: impl Into<Partition>) -> Path<Pixels> {
    let region = region.into();
    let vertices = std::mem::take(&mut path.vertices);
    if let Partition::Plane(plane) = region {
        for triangle in vertices.chunks_exact(3) {
            append_polygon(&mut path, &cut(triangle, plane));
        }
        return path;
    }
    let (planes, inside) = match region {
        Partition::Plane(plane) => (vec![plane], true),
        Partition::Body { pose, inside, .. } => {
            // A rounded source region, not a screen-aligned cut through the
            // shared material. Both owners keep the original outer border.
            let polygon = super::clipping::flatten(&rounded_rectangle(pose, 0.6));
            let area: f64 = polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .take(polygon.len())
                .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
                .sum();
            let sign = if area >= 0. { 1. } else { -1. };
            let mut planes: Vec<_> = polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .take(polygon.len())
                .filter_map(|(a, b)| {
                    let normal = [(b[1] - a[1]) * sign, (a[0] - b[0]) * sign];
                    (normal[0].abs() + normal[1].abs() > 1e-9).then_some(HalfPlane {
                        normal,
                        offset: normal[0] * a[0] + normal[1] * a[1],
                    })
                })
                .collect();
            if let Partition::Body { plane, .. } = region {
                planes.push(plane);
            }
            (planes, inside)
        }
    };
    let source_bounds = match region {
        Partition::Body { pose, .. } => pose,
        _ => unreachable!(),
    };
    for triangle in vertices.chunks_exact(3) {
        // Once the bodies separate, most triangles cannot reach the source.
        // Reject their bounds before walking its curved boundary.
        let outside = triangle
            .iter()
            .all(|v| (v.xy_position.x.as_f32() as f64) < source_bounds.left())
            || triangle.iter().all(|v| {
                (v.xy_position.x.as_f32() as f64) > source_bounds.left() + source_bounds.w
            })
            || triangle
                .iter()
                .all(|v| (v.xy_position.y.as_f32() as f64) < source_bounds.top())
            || triangle
                .iter()
                .all(|v| (v.xy_position.y.as_f32() as f64) > source_bounds.top() + source_bounds.h);
        if outside {
            if !inside {
                path.vertices_mut().extend_from_slice(triangle);
            }
            continue;
        }
        let mut remaining = triangle.to_vec();
        for plane in &planes {
            if remaining.len() < 3 {
                break;
            }
            // Most planes do not cross this polygon. Keep it in place instead
            // of cloning it into two temporary vectors for every source edge.
            let mut negative = false;
            let mut positive = false;
            for vertex in &remaining {
                let d = plane.distance([
                    vertex.xy_position.x.as_f32() as f64,
                    vertex.xy_position.y.as_f32() as f64,
                ]);
                negative |= d < 0.;
                positive |= d > 0.;
            }
            if !positive {
                continue;
            }
            if !negative {
                if !inside {
                    append_polygon(&mut path, &remaining);
                }
                remaining.clear();
                break;
            }
            if !inside {
                append_polygon(&mut path, &cut(&remaining, plane.reversed()));
            }
            remaining = cut(&remaining, *plane);
        }
        if inside {
            append_polygon(&mut path, &remaining);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::{partition, HalfPlane, Partition};
    use gpui::{Path, Pixels};
    use zork_liquid::{rounded_rectangle, Contour, Pose};
    fn area(path: &Path<Pixels>) -> f64 {
        path.vertices
            .chunks_exact(3)
            .map(|v| {
                let p = |i: usize| {
                    [
                        v[i].xy_position.x.as_f32() as f64,
                        v[i].xy_position.y.as_f32() as f64,
                    ]
                };
                let [a, b, c] = [p(0), p(1), p(2)];
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.
            })
            .sum()
    }
    #[test]
    fn curved_partition_keeps_all_material_without_an_internal_border() {
        let contour = Contour {
            loops: vec![rounded_rectangle(Pose::rect(0., 0., 280., 180., 32.), 0.6)],
            sampled_points: 0,
            full_grid_points: 0,
            used_fallback: false,
            revision: 0,
        };
        let fill = super::super::super::tessellation::fill_path(&contour).unwrap();
        let stroke =
            super::super::super::tessellation::stroke_path(&contour, crate::design::BORDER_WIDTH)
                .unwrap();
        for pose in [
            Pose::rect(80., -8., 64., 32., 16.),
            Pose::rect(100., 70., 64., 32., 16.),
        ] {
            for original in [&fill, &stroke] {
                let source = partition(
                    original.clone(),
                    Partition::Body {
                        pose,
                        plane: HalfPlane {
                            normal: [0., 0.],
                            offset: f64::INFINITY,
                        },
                        inside: true,
                    },
                );
                let body = partition(
                    original.clone(),
                    Partition::Body {
                        pose,
                        plane: HalfPlane {
                            normal: [0., 0.],
                            offset: f64::INFINITY,
                        },
                        inside: false,
                    },
                );
                assert!((area(&source) + area(&body) - area(original)).abs() < 0.1);
                assert!(area(&body) > 0.);
            }
        }
    }
    #[test]
    fn paint_owners_reconstruct_fill_and_original_border_without_a_seam() {
        let contour = Contour {
            loops: vec![rounded_rectangle(Pose::rect(0., 0., 280., 180., 32.), 0.6)],
            sampled_points: 0,
            full_grid_points: 0,
            used_fallback: false,
            revision: 0,
        };
        let fill = super::super::super::tessellation::fill_path(&contour).unwrap();
        let border =
            super::super::super::tessellation::stroke_path(&contour, crate::design::BORDER_WIDTH)
                .unwrap();
        for plane in [
            HalfPlane {
                normal: [0., 1.],
                offset: 40.,
            },
            HalfPlane {
                normal: [0.6, 0.8],
                offset: 160.,
            },
            HalfPlane {
                normal: [1., 0.],
                offset: f64::INFINITY,
            },
        ] {
            for original in [&fill, &border] {
                let source = partition(original.clone(), plane);
                let body = partition(original.clone(), plane.reversed());
                let error = (area(&source) + area(&body) - area(original)).abs();
                assert!(
                    error < 0.02,
                    "Ownership changed the filled area or added an internal border: {error}"
                );
                for (part, side) in [(&source, plane), (&body, plane.reversed())] {
                    assert!(part.vertices.iter().all(|v| side.distance([
                        v.xy_position.x.as_f32() as f64,
                        v.xy_position.y.as_f32() as f64
                    ]) <= 0.0001));
                }
            }
        }
    }
}

#[cfg(test)]
mod crossing_tests {
    use super::{partition, SourceMaterial};
    use crate::components::liquid::{Material, Options, Pose, Simulation, Surface};
    use gpui::{Path, Pixels};
    #[test]
    fn narrower_dialog_keeps_source_wings_with_the_page() {
        let source = Pose::rect(24., 244., 672., 48., 16.);
        let body = Pose::rect(90., 68., 540., 364., 32.);
        let mut simulation =
            Simulation::pair(source, body, Material::default(), Options::default());
        simulation.set_open(true);
        simulation.finish();
        let surface = Surface::new(simulation).unwrap();
        let binding = SourceMaterial::default();
        let foreground = binding.bind(&surface, source, None);
        let foreground_region = foreground.paint.0.borrow().partition.unwrap();
        let page_region = binding
            .resolve()
            .unwrap()
            .part
            .paint
            .0
            .borrow()
            .partition
            .unwrap();
        let area = |path: &Path<Pixels>| {
            path.vertices
                .chunks_exact(3)
                .map(|v| {
                    let a = v[0].xy_position;
                    let b = v[1].xy_position;
                    let c = v[2].xy_position;
                    ((b.x - a.x).as_f32() as f64 * (c.y - a.y).as_f32() as f64
                        - (b.y - a.y).as_f32() as f64 * (c.x - a.x).as_f32() as f64)
                        .abs()
                        / 2.
                })
                .sum::<f64>()
        };
        for original in [
            super::super::super::tessellation::fill_path(&surface.contour()).unwrap(),
            super::super::super::tessellation::stroke_path(
                &surface.contour(),
                crate::design::BORDER_WIDTH,
            )
            .unwrap(),
        ] {
            let page = partition(original.clone(), page_region);
            let foreground = partition(original.clone(), foreground_region);
            let tolerance =
                source.w.max(source.h).max(body.w).max(body.h).powi(2) * f32::EPSILON as f64 * 4.;
            assert!(
                (area(&original) - area(&page) - area(&foreground)).abs() < tolerance,
                "original={} page={} foreground={} error={}",
                area(&original),
                area(&page),
                area(&foreground),
                area(&original) - area(&page) - area(&foreground)
            );
            assert!(foreground
                .vertices
                .iter()
                .all(|v| v.xy_position.x.as_f32() >= body.left() as f32 - 0.01
                    && v.xy_position.x.as_f32() <= (body.left() + body.w) as f32 + 0.01));
        }
    }
}
