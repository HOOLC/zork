use super::*;

#[test]
fn departing_bubbles_preserve_existing_material_and_retire() {
    use super::departure::{Departures, Origin};
    let body = Pose::rect(18., 168., 320., 76., 18.);
    let member = Pose::rect(34., 160., 32., 32., 16.);
    for origin in [Origin::Button, Origin::Composer] {
        let mut sim =
            Simulation::compound(&[body, member], 4., Material::default(), Options::default());
        sim.finish();
        let original = trace(&sim).unwrap().svg_path();
        let mass = sim.mass();
        let identity = sim.particles().map(|p| (p.id, p.mass)).collect::<Vec<_>>();
        let mut departures = Departures::default();
        departures.emit(origin, "共享材料分离", body, &mut sim);
        assert_eq!(
            sim.particles()
                .take(identity.len())
                .map(|p| (p.id, p.mass))
                .collect::<Vec<_>>(),
            identity
        );
        let mut highest = body.top();
        for _ in 0..720 {
            departures.advance(FIXED_DT, false, &mut sim);
            let mut poses = vec![body, member];
            poses.extend(departures.targets(body));
            sim.set_compound_targets(&poses);
            sim.advance(FIXED_DT, false);
            departures.prepare(&sim);
            for bubble in departures.bubbles(body, &sim) {
                assert_eq!(
                    bubble.text_width, 186.,
                    "line layout must not follow the spring width"
                );
                highest = highest.min(bubble.pose.top());
            }
            trace(&sim).expect("departing compound retains closed contours");
        }
        assert!(highest < 0., "bubble should fly out of the preview");
        assert!(!departures.active());
        assert_eq!(departures.render_errors(), 0);
        sim.finish();
        assert_eq!(sim.mass(), mass);
        assert_eq!(trace(&sim).unwrap().svg_path(), original);
        departures.emit(origin, "减弱动态效果", body, &mut sim);
        departures.advance(0., true, &mut sim);
        assert!(!departures.active());
        assert_eq!(sim.particles().count(), identity.len());
    }
}

#[test]
fn departing_burst_and_color_failure_do_not_abort_or_replace_existing_groups() {
    use super::departure::{Departures, Origin};
    let body = Pose::rect(18., 168., 320., 76., 18.);
    let member = Pose::rect(34., 160., 32., 32., 16.);
    for origin in [Origin::Button, Origin::Composer] {
        let mut sim =
            Simulation::compound(&[body, member], 4., Material::default(), Options::default());
        let identity = sim.particles().map(|p| (p.id, p.mass)).collect::<Vec<_>>();
        let mut departures = Departures::default();
        let mut peak = 0;
        for tick in 0..720 {
            if tick < 128 && tick % 8 == 0 {
                departures.emit(origin, "快速连续发送", body, &mut sim);
            }
            departures.advance(FIXED_DT, false, &mut sim);
            let mut targets = vec![body, member];
            targets.extend(departures.targets(body));
            peak = peak.max(targets.len());
            assert!(targets.len() <= 6);
            sim.set_compound_targets(&targets);
            sim.tick();
            departures.prepare(&sim);
            assert_eq!(
                sim.particles()
                    .take(identity.len())
                    .map(|p| (p.id, p.mass))
                    .collect::<Vec<_>>(),
                identity
            );
        }
        assert_eq!(peak, 6);
        assert!(!departures.active());
        assert_eq!(departures.render_errors(), 0);
        assert_eq!(sim.particles().count(), identity.len());
    }
    // Fault injection: the optional color layer hits the contour bounds guard.
    // Rendering must survive and recover when the material returns to bounds.
    let mut sim =
        Simulation::compound(&[body, member], 4., Material::default(), Options::default());
    let mut departures = Departures::default();
    departures.emit(Origin::Button, "颜色层恢复", body, &mut sim);
    sim.set_compound_targets(&[body, member, Pose::rect(0., 0., 1e6, 1e6, 18.)]);
    sim.finish();
    departures.prepare(&sim);
    assert_eq!(departures.render_errors(), 1);
    assert!(departures.bubbles(body, &sim)[0].accent.is_none());
    sim.set_compound_targets(&[body, member, Pose::rect(128., 38., 210., 52., 18.)]);
    sim.finish();
    departures.prepare(&sim);
    assert!(departures.bubbles(body, &sim)[0].accent.is_some());
}
