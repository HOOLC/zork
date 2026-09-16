//! Paint ownership of one connected material, independent of platform layers.
use crate::{Point, Pose};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MaterialPartition {
    pub normal: Point,
    pub offset: f64,
}
impl MaterialPartition {
    /// The source owns the negative side. Support extents assign an overlapping
    /// large body the foreground while keeping each separated parcel intact.
    /// Crossing aspect ratios need a bounded foreground region: a half-plane
    /// alone can assign the wide source's side wings to a narrower dialog.
    pub fn crossing(source: Pose, body: Pose) -> bool {
        ((source.w > body.w && source.h < body.h) || (source.w < body.w && source.h > body.h))
            && (source.cx - body.cx).abs() < (source.w + body.w) / 2.
            && (source.cy - body.cy).abs() < (source.h + body.h) / 2.
    }
    pub fn source(source: Pose, body: Pose, open: bool) -> Self {
        Self::along(
            source,
            body,
            open,
            crate::geometry::contact_axis(source, body),
        )
    }
    pub(crate) fn along(source: Pose, body: Pose, open: bool, normal: Point) -> Self {
        let delta = [body.cx - source.cx, body.cy - source.cy];
        let distance = delta[0] * normal[0] + delta[1] * normal[1];
        let extent = |p: Pose| (normal[0].abs() * p.w + normal[1].abs() * p.h) / 2.;
        let center = normal[0] * source.cx + normal[1] * source.cy;
        let mut offset = center + (distance + extent(source) - extent(body)) / 2.;
        if !open {
            // The returning parcel hands its shared surface back to the source
            // as its remaining protrusion disappears. A center bisector would
            // leave an opaque half-button over the original caption throughout
            // the spring's small settling oscillations.
            let protrusion = (delta[0].abs() + (body.w - source.w) / 2.)
                .max(delta[1].abs() + (body.h - source.h) / 2.)
                .max(0.);
            let remaining = (protrusion / (source.short() / 2.).max(0.5)).min(1.);
            offset = if remaining == 0. {
                f64::INFINITY
            } else {
                let edge = center + extent(source);
                edge + (offset - edge) * remaining
            };
        }
        Self { normal, offset }
    }
    pub fn distance(self, point: Point) -> f64 {
        self.normal[0] * point[0] + self.normal[1] * point[1] - self.offset
    }
    pub fn reversed(self) -> Self {
        Self {
            normal: self.normal.map(|v| -v),
            offset: -self.offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separated_parcels_have_distinct_owners_and_one_shared_boundary() {
        let source = Pose::rect(30., 20., 120., 32., 16.);
        let body = Pose::rect(30., 64., 280., 200., 32.);
        let page = MaterialPartition::source(source, body, true);
        let popup = page.reversed();
        assert!(page.distance([source.cx, source.cy]) < 0.);
        assert!(popup.distance([body.cx, body.cy]) < 0.);
        for point in [
            [source.left(), source.top()],
            [source.left() + source.w, source.top()],
            [source.left(), source.top() + source.h],
            [source.left() + source.w, source.top() + source.h],
        ] {
            assert!(
                page.distance(point) < 0.,
                "The popup took a part of the separated source: {point:?}"
            );
        }
        for x in [0., 60., 180., 300.] {
            for y in [0., 30., 60., 120., 280.] {
                assert_eq!(page.distance([x, y]), -popup.distance([x, y]));
            }
        }
    }
    #[test]
    fn returning_parcel_leaves_all_fused_material_with_the_page_source() {
        let source = Pose::rect(0., 0., 128., 32., 16.);
        let body = Pose::rect(2., 2., 124., 28., 14.);
        let page = MaterialPartition::source(source, body, false);
        assert_eq!(page.offset, f64::INFINITY);
        assert!(page.reversed().distance([body.cx, body.cy]) > 0.);
        assert!(MaterialPartition::source(source, body, true)
            .offset
            .is_finite());
    }
    #[test]
    fn fusion_settling_does_not_leave_a_foreground_half_over_the_source_caption() {
        let source = Pose::rect(0., 0., 128., 32., 16.);
        // Both sides of the resting pose occur in an underdamped return.
        for (dx, dy, dw, dh) in [
            (-1.482, -0.034, -1.298, -1.472),
            (-0.269, 0.01, -1.092, -2.471),
            (0.1, 0.1, 0.1, 0.1),
        ] {
            let mut body = source;
            body.cx += dx;
            body.cy += dy;
            body.w += dw;
            body.h += dh;
            for normal in [[1., 0.], [0., 1.], [-1., 0.], [0., -1.]] {
                let page = MaterialPartition::along(source, body, false, normal);
                for x in [source.cx - 30., source.cx + 30.] {
                    for y in [source.cy - 10., source.cy + 10.] {
                        assert!(
                            page.distance([x, y]) < 0.,
                            "Caption remains covered: {page:?}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn page_transport_preserves_the_partition_in_local_coordinates() {
        let source = Pose::rect(30., 20., 120., 32., 16.);
        let body = Pose::rect(30., 64., 280., 200., 32.);
        let original = MaterialPartition::source(source, body, true);
        let shift = |mut pose: Pose| {
            pose.cx += 13.;
            pose.cy -= 90.;
            pose
        };
        let shifted = MaterialPartition::source(shift(source), shift(body), true);
        assert!((original.distance([70., 40.]) - shifted.distance([83., -50.])).abs() < 1e-8);
    }
    #[test]
    fn layout_transport_updates_the_source_contour_before_the_next_physics_step() {
        let source = Pose::rect(20., 50., 120., 32., 16.);
        let target = Pose::rect(160., 300., 280., 200., 32.);
        let mut simulation =
            crate::Simulation::pair(source, target, Default::default(), Default::default());
        simulation.set_open(true);
        for _ in 0..60 {
            simulation.advance(1. / 240., false);
        }
        let before = simulation.source_pose();
        let mut shifted = source;
        shifted.cy -= 120.;
        let mut surface = crate::Surface::new(simulation).unwrap();
        surface.simulation.layout_pair(shifted, target);
        let after = surface.simulation.source_pose();
        assert!((after.cy - before.cy + 120.).abs() < 1e-8);
        assert_eq!([after.vx, after.vy], [before.vx, before.vy]);
        surface.prepare();
        assert!(surface.contour().contains([after.cx, after.cy]));
    }
}
