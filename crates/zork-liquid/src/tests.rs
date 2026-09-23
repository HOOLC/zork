use super::*;
use serde_json::Value;

fn number(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

#[test]
fn elastic_travel_stays_in_pixels_across_sizes_and_reversals() {
    for size in [24., 64., 256., 640., 1280.] {
        for anchor in [[0., 0.], [0.5, 0.5]] {
            let start = Pose::rect(0., 0., size, size * 0.65, 12.);
            let end = Pose::rect(160., 96., size * 1.5, size * 1.2, 20.);
            let mut sim = Simulation::new(
                start,
                Material::default(),
                Options {
                    anchor,
                    ..Options::default()
                },
            );
            sim.finish();
            sim.set_target(end);
            for _ in 0..24 {
                sim.tick();
            }
            let reversed_from = sim.pose();
            sim.set_target(start);
            assert_eq!(
                sim.pose(),
                reversed_from,
                "retarget changed velocity or position"
            );
            for (from, target) in [(reversed_from, start), (start, end)] {
                sim.set_target(target);
                let a = [from.cx, from.cy, from.w, from.h];
                let b = [target.cx, target.cy, target.w, target.h];
                let mut excursion = [0_f64; 4];
                for _ in 0..1440 {
                    sim.tick();
                    let p = sim.pose();
                    for (i, v) in [p.cx, p.cy, p.w, p.h].into_iter().enumerate() {
                        excursion[i] = excursion[i].max(a[i].min(b[i]) - v).max(v - a[i].max(b[i]));
                    }
                    if !sim.moving() {
                        break;
                    }
                }
                assert!(!sim.moving(), "size={size}, anchor={anchor:?}");
                assert!(
                    excursion.iter().all(|v| *v < 8.),
                    "size={size}, anchor={anchor:?}, excursion={excursion:?}"
                );
            }
        }
    }
}

#[test]
fn expanding_panels_keep_rectangular_corners_at_speed() {
    let source = Pose::rect(688., 10., 64., 32., 16.);
    let target = Pose::rect(114., 39., 540., 522., 24.);
    for anchor in [[0., 0.], [0.5, 0.5]] {
        let mut sim = Simulation::pair(
            source,
            target,
            Material::default(),
            Options {
                anchor,
                capacity: target.w * target.h,
                ..Options::default()
            },
        );
        sim.finish();
        for open in [true, false] {
            sim.set_open(open);
            for tick in 0..180 {
                sim.tick();
                let p = sim.pose();
                if p.short() >= 140. && tick % 4 == 0 {
                    let inset = (p.short() * 0.12).min(30.);
                    let point = [p.left() + inset, p.top() + p.h - inset];
                    assert!(trace(&sim).unwrap().contains(point),
                        "fast panel lost its rectangular corner: open={open}, tick={tick}, pose={p:?}");
                }
            }
            sim.finish();
        }
    }
}

#[test]
fn paired_modal_preserves_its_source_and_visible_panel_bounds() {
    let source = Pose::rect(688., 10., 64., 32., 16.);
    for height in [600., 837.] {
        let target = Pose::rect(114., 32., 540., (height - 64_f64).min(660.), 24.);
        let mut sim = Simulation::pair(
            source,
            target,
            Material::default(),
            Options {
                anchor: [0.5, 0.5],
                capacity: target.w * target.h,
                ..Default::default()
            },
        );
        sim.finish();
        for open in [true, false] {
            sim.set_open(open);
            for tick in 0..1440 {
                sim.tick();
                let carrier = sim.source_pose();
                assert!((carrier.cx - source.cx).abs() < 4.);
                assert!((carrier.cy - source.cy).abs() < 4.);
                assert!((carrier.w - source.w).abs() < 4.);
                assert!((carrier.h - source.h).abs() < 4.);
                let panel = sim.pose();
                if open && panel.w > target.w * 0.9 {
                    assert!(panel.left() >= 0. && panel.left() + panel.w <= 768.);
                    assert!(panel.top() >= 0. && panel.top() + panel.h <= height);
                }
                if [12, 36, 72].contains(&tick) {
                    let contour = trace(&sim).unwrap();
                    assert!(contour.contains([carrier.cx, carrier.cy]));
                    assert!(!contour.used_fallback);
                }
                if !sim.moving() {
                    break;
                }
            }
            assert!(!sim.moving());
        }
        sim.set_open(true);
        for _ in 0..24 {
            sim.tick();
        }
        let before = (sim.pose(), sim.source_pose());
        sim.set_open(false);
        assert_eq!((sim.pose(), sim.source_pose()), before);
    }
}

#[test]
fn pixel_rebound_limits_panel_motion_and_keeps_sparse_contours() {
    let start = Pose::rect(710., 10., 64., 32., 16.);
    let end = Pose::rect(129., 88.5, 540., 660., 24.);
    for material in [
        Material::default(),
        Material::ordinary(),
        Material {
            flow: 0.4,
            damping: 0.4,
            budget: 64,
            ..Material::default()
        },
    ] {
        let mut sim = Simulation::new(
            start,
            material,
            Options {
                anchor: [0.5, 0.5],
                capacity: end.w * end.h,
                ..Options::default()
            },
        );
        sim.finish();
        for (from, target) in [(start, end), (end, start)] {
            sim.set_target(target);
            let mut overshoot = [0_f64; 4];
            for tick in 0..1440 {
                sim.tick();
                let p = sim.pose();
                for (i, (a, b, value)) in [
                    (from.cx, target.cx, p.cx),
                    (from.cy, target.cy, p.cy),
                    (from.w, target.w, p.w),
                    (from.h, target.h, p.h),
                ]
                .into_iter()
                .enumerate()
                {
                    overshoot[i] = overshoot[i].max((value - b) * (b - a).signum());
                }
                if [12, 36, 72].contains(&tick) {
                    let sparse = trace(&sim).unwrap();
                    let dense = trace_dense(&sim).unwrap();
                    assert!(!sparse.used_fallback);
                    assert_eq!(sparse.loops.len(), dense.loops.len());
                    for curve in dense.loops.iter().flatten().step_by(2) {
                        let p = curve.at(0.5);
                        let a = curve.at(0.49);
                        let b = curve.at(0.51);
                        let normal = super::geometry::unit([a[1] - b[1], b[0] - a[0]]);
                        for offset in [-1., 1.] {
                            let point = [p[0] + normal[0] * offset, p[1] + normal[1] * offset];
                            assert_eq!(sparse.contains(point), dense.contains(point));
                        }
                    }
                }
                if !sim.moving() {
                    break;
                }
            }
            assert!(!sim.moving());
            assert!(
                overshoot.iter().all(|v| *v < 8.),
                "panel overshoot: {overshoot:?}"
            );
        }
        sim.set_target(end);
        for _ in 0..24 {
            sim.tick();
        }
        let before = sim.pose();
        sim.set_target(start);
        assert_eq!(
            sim.pose(),
            before,
            "reversal must preserve position and velocity"
        );
    }
}

#[test]
fn gallery_parameters_keep_sparse_contours_equivalent_to_dense() {
    for (flow, damping, smoothing, adhesion, budget) in [
        (0.12, 1., 0.6, 0.72, 12),
        (0.08, 1., 0.5, 0.72, 12),
        (0.4, 0.4, 0., 1.2, 6),
        (0.4, 0.4, 1., 1.2, 64),
        (0., 2., 0.5, 0., 24),
    ] {
        let material = Material {
            flow,
            damping,
            smoothing,
            adhesion,
            budget,
            ..Material::default()
        };
        let body = Pose::rect(18., 168., 320., 76., 18.);
        let source = Pose::rect(308., 214., 24., 24., 12.);
        let mut sim = Simulation::compound(&[body, source], 4., material, Options::default());
        sim.set_compound_targets(&[body, Pose::rect(128., 38., 210., 52., 18.)]);
        for tick in 0..100 {
            sim.tick();
            if ![0, 12, 36, 72].contains(&tick) {
                continue;
            }
            let sparse = trace(&sim).unwrap();
            let dense = super::contour::trace_dense(&sim).unwrap();
            assert!(
                !sparse.used_fallback,
                "interactive parameter forced a dense frame: {material:?}, tick={tick}"
            );
            assert!(sparse.sampled_points < dense.sampled_points / 4);
            assert_eq!(
                sparse.loops.len(),
                dense.loops.len(),
                "topology: {material:?}, tick={tick}"
            );
            for curve in dense.loops.iter().flatten().step_by(2) {
                let p = curve.at(0.5);
                let a = curve.at(0.49);
                let b = curve.at(0.51);
                let normal = super::geometry::unit([a[1] - b[1], b[0] - a[0]]);
                for offset in [-1., 1.] {
                    let probe = [p[0] + normal[0] * offset, p[1] + normal[1] * offset];
                    assert_eq!(
                        sparse.contains(probe),
                        dense.contains(probe),
                        "boundary: {material:?}, tick={tick}, probe={probe:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn ordinary_navigation_travel_keeps_sparse_boundary_equivalent_to_dense() {
    for width in [240., 492.] {
        let start = Pose::rect(0., 0., width, 32., 14.);
        let mut sim = Simulation::new(start, Material::ordinary(), Options::default());
        sim.finish();
        for y in [160., -64., 96.] {
            sim.set_travel_target(Pose::rect(0., y, width, 32., 14.));
            for tick in 0..96 {
                sim.tick();
                if ![0, 12, 36, 72].contains(&tick) {
                    continue;
                }
                let sparse = trace(&sim).unwrap();
                let dense = trace_dense(&sim).unwrap();
                assert!(!sparse.used_fallback, "width={width}, tick={tick}");
                assert!(sparse.sampled_points < dense.sampled_points / 4);
                assert_eq!(sparse.loops.len(), dense.loops.len());
                for curve in dense.loops.iter().flatten().step_by(2) {
                    let p = curve.at(0.5);
                    let a = curve.at(0.49);
                    let b = curve.at(0.51);
                    let normal = super::geometry::unit([a[1] - b[1], b[0] - a[0]]);
                    for offset in [-1., 1.] {
                        let probe = [p[0] + normal[0] * offset, p[1] + normal[1] * offset];
                        assert_eq!(sparse.contains(probe), dense.contains(probe));
                    }
                }
            }
        }
    }
}

#[test]
fn departing_color_contour_survives_gallery_parameters() {
    for width in [240., 760.] {
        for (flow, damping, smoothing, budget) in [
            (0.08, 1., 0.6, 12),
            (0.12, 1., 0.6, 12),
            (0.4, 0.4, 0., 6),
            (0.4, 0.4, 1., 64),
            (0., 2., 0.5, 24),
        ] {
            let material = Material {
                flow,
                damping,
                smoothing,
                budget,
                ..Material::default()
            };
            let body = Pose::rect(18., 168., width, 112., 18.);
            let member = Pose::rect(34., 160., 32., 32., 16.);
            let mut sim = Simulation::compound(&[body, member], 4., material, Options::default());
            let source = Pose::rect(
                body.left() + body.w - 30.,
                body.top() + body.h - 30.,
                24.,
                24.,
                12.,
            );
            let index = sim.append_compound_group(source);
            sim.set_compound_targets(&[
                body,
                member,
                Pose::rect(body.left() + body.w - 210., 38., 210., 52., 18.),
            ]);
            for tick in 0..80 {
                sim.tick();
                if tick % 4 == 0 {
                    trace(&sim.group_snapshot(index)).unwrap_or_else(|error| panic!("color contour: width={width}, material={material:?}, tick={tick}: {error:?}"));
                }
            }
        }
    }
}

fn close(actual: f64, expected: f64, tolerance: f64, label: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{label}: actual={actual}, expected={expected}, difference={}",
        (actual - expected).abs()
    );
}

#[test]
fn matches_fixed_javascript_reference_without_ui_rebound_limit() {
    let reference = Material {
        rebound_limit: None,
        ..Material::default()
    };
    let data: Value = serde_json::from_str(include_str!("fixtures/reference12.json")).unwrap();
    let mut worst_pose = 0_f64;
    let mut worst_particle = 0_f64;
    let mut worst_field = 0_f64;
    let mut worst_contour = 0_f64;
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let kind = case["kind"].as_str().unwrap();
        let initial: Pose = serde_json::from_value(case["initial"].clone()).unwrap();
        let target: Option<Pose> = serde_json::from_value(case["target"].clone()).unwrap();
        let options = Options {
            seed: case["options"]["seed"].as_u64().unwrap() as u32,
            anchor: [number(&case["anchor"][0]), number(&case["anchor"][1])],
            capacity: number(&case["options"]["capacity"]),
            constraints: case
                .get("constraints")
                .map(|v| serde_json::from_value(v.clone()).unwrap())
                .unwrap_or_default(),
            ..Options::default()
        };
        let mut simulation = match kind {
            "pair" => Simulation::pair(initial, target.unwrap(), reference, options),
            "split" => Simulation::split(
                initial,
                &case["targets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(case["fractions"].as_array().unwrap())
                    .map(|(p, f)| (serde_json::from_value(p.clone()).unwrap(), number(f)))
                    .collect::<Vec<_>>(),
                reference,
                options,
            ),
            _ => Simulation::new(initial, reference, options),
        };
        for tick in 0..=720 {
            for action in case["actions"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|a| a["tick"].as_u64() == Some(tick))
            {
                let open = action["open"].as_bool().unwrap();
                if kind == "single" {
                    simulation.set_target(if open { target.unwrap() } else { initial });
                } else {
                    simulation.set_open(open);
                }
            }
            if let Some(frame) = case["frames"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["tick"].as_u64() == Some(tick))
            {
                for (g, expected) in simulation
                    .groups
                    .iter()
                    .zip(frame["groups"].as_array().unwrap())
                {
                    let pose = serde_json::to_value(g.body.pose()).unwrap();
                    for key in ["cx", "cy", "w", "h", "r", "vx", "vy", "vw", "vh"] {
                        let a = number(&pose[key]);
                        let b = number(&expected["pose"][key]);
                        worst_pose = worst_pose.max((a - b).abs());
                        close(a, b, 0.002, &format!("{name} tick={tick} pose {key}"));
                    }
                    close(g.mass(), number(&expected["mass"]), 0.01, name);
                    close(g.area, number(&expected["area"]), 0.02, name);
                    for (p, expected) in g
                        .particles
                        .iter()
                        .zip(expected["particles"].as_array().unwrap())
                    {
                        for (i, a) in [p.id as f64, p.x, p.y, p.qx, p.qy, p.vx, p.vy, p.mass]
                            .into_iter()
                            .enumerate()
                        {
                            let b = number(&expected[i]);
                            worst_particle = worst_particle.max((a - b).abs());
                            close(
                                a,
                                b,
                                0.004,
                                &format!("{name} tick={tick} particle {} field={i}", p.id),
                            );
                        }
                    }
                }
                let sampler = field::Sampler::new(&simulation);
                for point in frame["field"].as_array().unwrap() {
                    let a = sampler.value(number(&point[0]), number(&point[1]));
                    let b = number(&point[2]);
                    worst_field = worst_field.max((a - b).abs());
                    close(a, b, 0.002, &format!("{name} tick={tick} field"));
                }
                let numbers = |s: &str| {
                    s.split(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+')))
                        .filter(|s| !s.is_empty())
                        .map(|s| s.parse::<f64>().unwrap())
                        .collect::<Vec<_>>()
                };
                let actual = numbers(&trace(&simulation).unwrap().svg_path());
                let expected = numbers(frame["contour"].as_str().unwrap());
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "{name} tick={tick} contour topology/control count"
                );
                for (a, b) in actual.into_iter().zip(expected) {
                    worst_contour = worst_contour.max((a - b).abs());
                    close(
                        a,
                        b,
                        0.003,
                        &format!("{name} tick={tick} cubic control point"),
                    );
                }
            }
            simulation.tick();
        }
    }
    println!("reference max errors: pose={worst_pose:.8}, particle={worst_particle:.8}, field={worst_field:.8}, contour={worst_contour:.8}");
}

#[test]
fn retarget_preserves_samples_mass_and_velocity() {
    let start = Pose::rect(18., 18., 116., 32., 16.);
    let target = Pose::rect(18., 18., 540., 414., 32.);
    let mut s = Simulation::new(
        start,
        Material::default(),
        Options {
            anchor: [0., 0.],
            capacity: 540. * 414.,
            ..Options::default()
        },
    );
    let mass = s.mass();
    let ids: Vec<_> = s.particles().map(|p| p.id).collect();
    s.set_target(target);
    for _ in 0..22 {
        s.tick();
    }
    let pose = s.pose();
    let particles: Vec<_> = s
        .particles()
        .map(|p| (p.id, p.x, p.y, p.vx, p.vy))
        .collect();
    s.set_target(start);
    assert_eq!(s.pose(), pose);
    assert_eq!(
        s.particles()
            .map(|p| (p.id, p.x, p.y, p.vx, p.vy))
            .collect::<Vec<_>>(),
        particles
    );
    for _ in 0..1440 {
        if !s.moving() {
            break;
        }
        s.tick();
    }
    assert!(!s.moving());
    assert_eq!(ids, s.particles().map(|p| p.id).collect::<Vec<_>>());
    close(s.mass(), mass, 1e-8, "mass");
    close(s.pose().w, 116., 0.001, "settled width");
}

#[test]
fn refresh_cadence_and_reduced_motion() {
    let base = Simulation::new(
        Pose::rect(0., 0., 124., 32., 16.),
        Material::default(),
        Options::default(),
    );
    let mut outcomes = Vec::new();
    for hz in [30, 60, 120, 240] {
        let mut s = base.clone();
        s.set_target(Pose::rect(80., 10., 320., 208., 20.));
        for _ in 0..hz / 2 {
            s.advance(1. / hz as f64, false);
        }
        outcomes.push(s.pose());
    }
    for p in &outcomes[1..] {
        close(p.cx, outcomes[0].cx, 1e-10, "cadence x");
        close(p.w, outcomes[0].w, 1e-10, "cadence width");
    }
    let mut s = base;
    s.set_target(Pose::rect(0., 0., 320., 208., 20.));
    s.advance(0.01, true);
    assert!(!s.moving());
    close(s.pose().h, 208., 1e-10, "reduced height");
}

#[test]
fn modal_playback_retires_the_invisible_tail_without_cutting_reversals() {
    let source = Pose::rect(638., 9., 64., 32., 16.);
    let target = Pose::rect(129., 96., 540., 646., 32.);
    let mut simulation = Simulation::pair(source, target, Material::default(), Options::default());
    simulation.finish();
    let mass = simulation.mass();
    let particles = simulation.particles().map(|p| p.id).collect::<Vec<_>>();
    simulation.set_open(true);
    for _ in 0..12 {
        simulation.advance(1. / 120., false);
    }
    let moving = simulation.pose();
    assert!(moving.vx.abs() + moving.vy.abs() > 100.);
    simulation.set_open(false);
    assert_eq!(
        simulation.pose(),
        moving,
        "reversal reset the live velocity"
    );
    simulation.advance(1. / 120., false);
    assert!(
        simulation.moving(),
        "target crossing prematurely stopped the animation"
    );
    for open in [false, true, false] {
        simulation.set_open(open);
        let mut last = simulation.pose();
        let mut frames = 0;
        while simulation.moving() {
            last = simulation.pose();
            simulation.advance(1. / 120., false);
            frames += 1;
            assert!(
                frames < 180,
                "fixed modal fixture retained an invisible numeric tail"
            );
        }
        let final_pose = simulation.pose();
        let edge = (last.cx - final_pose.cx).abs()
            + (last.cy - final_pose.cy).abs()
            + 0.5 * ((last.w - final_pose.w).abs() + (last.h - final_pose.h).abs());
        assert!(edge < 0.15, "visible jump at retirement: {edge}");
        assert_eq!(final_pose, if open { target } else { source });
        assert_eq!(simulation.mass(), mass);
        assert_eq!(
            simulation.particles().map(|p| p.id).collect::<Vec<_>>(),
            particles
        );
    }
}

#[test]
fn rebound_contour_hit_and_fallback() {
    let start = Pose::rect(18., 18., 116., 32., 16.);
    let target = Pose::rect(18., 18., 402., 414., 32.);
    let mut s = Simulation::new(
        start,
        Material::default(),
        Options {
            anchor: [0., 0.],
            capacity: 402. * 414.,
            ..Options::default()
        },
    );
    s.set_target(target);
    s.finish();
    s.set_target(start);
    for _ in 0..64 {
        s.tick();
    }
    assert!(s.pose().h < 32.);
    let path = trace(&s).unwrap();
    assert!(!path.used_fallback);
    assert!(path.sampled_points < path.full_grid_points / 4);
    assert!(path.contains([s.pose().cx, s.pose().cy]));
    assert!(!path.contains([s.pose().cx, s.pose().top() + 32.]));
    let mut m = s.material();
    m.surface_detail = 0.12;
    s.configure(m);
    assert!(trace(&s).unwrap().used_fallback);
}

#[test]
fn compound_members_keep_motion_when_neighbors_join_and_leave() {
    let body = Pose::rect(0., 140., 420., 76., 18.);
    let member = Pose::rect(20., 130., 32., 32., 16.);
    let mut sim =
        Simulation::compound(&[body, member], 4., Material::default(), Options::default());
    sim.finish();
    sim.set_compound_group_target(1, Pose::rect(0., 90., 180., 32., 16.));
    for _ in 0..18 {
        sim.tick();
    }
    let before = sim.group_snapshot(1);
    assert!(before.particles().any(|p| p.vx != 0. || p.vy != 0.));
    let particles = |s: &Simulation| {
        s.particles()
            .map(|p| (p.id, p.mass, p.x, p.y, p.vx, p.vy))
            .collect::<Vec<_>>()
    };
    sim.insert_compound_group(1, member);
    assert_eq!(before.pose(), sim.group_pose(2));
    assert_eq!(particles(&before), particles(&sim.group_snapshot(2)));
    sim.remove_compound_group(1);
    assert_eq!(before.pose(), sim.group_pose(1));
    assert_eq!(particles(&before), particles(&sim.group_snapshot(1)));
    // Transport budgets are enforced by their host. A native membership
    // snapshot must not turn the solver's growable storage into an assertion.
    for _ in 0..300 {
        sim.append_compound_group(member);
    }
    assert_eq!(before.pose(), sim.group_pose(1));
    assert_eq!(member, sim.group_pose(301));
}

#[test]
fn compound_reversal_and_aperture_share_one_contour() {
    let body = Pose::rect(18., 130., 420., 112., 18.);
    let idle = [
        body,
        Pose::rect(40., 122., 32., 32., 16.),
        Pose::rect(62., 122., 32., 32., 16.),
    ];
    let active = [
        Pose::rect(18., 166., 420., 76., 18.),
        Pose::rect(18., 131., 180., 32., 16.),
        Pose::rect(18., 96., 160., 32., 16.),
    ];
    let mut s = Simulation::compound(
        &idle,
        4.,
        Material::default(),
        Options {
            anchor: [0., 1.],
            ..Default::default()
        },
    );
    let mass = s.mass();
    s.set_compound_targets(&active);
    for _ in 0..35 {
        s.tick();
    }
    let before = s
        .particles()
        .map(|p| (p.id, p.mass, p.x, p.y, p.vx, p.vy))
        .collect::<Vec<_>>();
    let pose = s.pose();
    s.set_compound_targets(&idle);
    assert_eq!(pose, s.pose());
    assert_eq!(
        before,
        s.particles()
            .map(|p| (p.id, p.mass, p.x, p.y, p.vx, p.vy))
            .collect::<Vec<_>>()
    );
    for i in 0..320 {
        s.tick();
        if i % 20 == 0 {
            let contour = trace(&s).unwrap();
            assert!(contour.contains([s.pose().cx, s.pose().cy]));
            for slot in 1..3 {
                let p = s.group_pose(slot);
                assert!(contour.contains([p.cx, p.cy]));
            }
        }
    }
    close(s.mass(), mass, 1e-8, "compound mass");
    let mut surface = Surface::new(s).unwrap();
    let slot = Pose::rect(300., 135., 80., 4., 2.);
    let point = [slot.cx, slot.cy];
    assert!(surface.contour().contains(point));
    surface.set_cutouts(vec![rounded_rectangle(slot, 0.)]);
    assert!(!surface.contour().contains(point));
    assert!(surface.contour().contains([200., 180.]));
    surface.set_cutouts(vec![]);
    assert!(surface.contour().contains(point));
}

#[test]
fn moving_composer_member_does_not_move_distant_bottom_edge() {
    let body = Pose::rect(18., 168., 524., 76., 18.);
    let idle = [
        body,
        Pose::rect(40., 160., 32., 32., 16.),
        Pose::rect(58., 200., 32., 32., 16.),
        Pose::rect(58., 200., 32., 32., 16.),
    ];
    let mut target = idle;
    target[1] = Pose::rect(18., 133., 210., 32., 16.);
    let mut sim = Simulation::compound(
        &idle,
        4.,
        Material::default(),
        Options {
            anchor: [0., 1.],
            ..Default::default()
        },
    );
    sim.finish();
    let stationary = sim.groups[0].particles.clone();
    let edge = |sim: &Simulation, x: f64| {
        let sampler = super::field::Sampler::new(sim);
        let (mut lo, mut hi) = (body.top() + body.h - 12., body.top() + body.h + 12.);
        for _ in 0..25 {
            let mid = (lo + hi) * 0.5;
            if sampler.value(x, mid) > 0. {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) * 0.5
    };
    let xs = [58., 74., 90., 140., 280., 480.];
    let resting = xs.map(|x| edge(&sim, x));
    let mut maximum = 0_f64;
    sim.set_compound_targets(&target);
    for tick in 0..240 {
        if tick == 34 {
            sim.set_compound_targets(&idle);
        }
        if tick == 57 {
            sim.set_compound_targets(&target);
        }
        sim.tick();
        if tick % 4 == 0 {
            for (x, rest) in xs.into_iter().zip(resting) {
                maximum = maximum.max((edge(&sim, x) - rest).abs());
            }
        }
    }
    eprintln!("moving member: maximum distant bottom displacement = {maximum:.6} px");
    assert!(
        maximum < 0.02,
        "member motion leaks into the stationary composer bottom: {maximum}"
    );
    for (before, after) in stationary.iter().zip(&sim.groups[0].particles) {
        assert_eq!(
            (before.id, before.x, before.y, before.vx, before.vy),
            (after.id, after.x, after.y, after.vx, after.vy)
        );
    }
    // A material change must wake even a previously stationary compound body.
    sim.finish();
    sim.configure(Material {
        smoothing: 1.,
        budget: 24,
        ..sim.material()
    });
    assert!(sim.moving());
    let mut settled = sim.clone();
    settled.finish();
    sim.tick();
    for (actual, expected) in sim.groups[0]
        .particles
        .iter()
        .zip(&settled.groups[0].particles)
    {
        assert!((actual.qx - expected.qx).abs() < 1e-8);
        assert!((actual.qy - expected.qy).abs() < 1e-8);
    }
}

#[test]
fn paired_dialog_preserves_separation_and_fusion_necks() {
    let source = Pose::rect(638., 10., 64., 32., 16.);
    let target = Pose::rect(129., 95., 540., 646., 32.);
    let mut simulation = Simulation::pair(
        source,
        target,
        Material::default(),
        Options {
            anchor: [0.5, 0.5],
            capacity: target.w * target.h,
            ..Default::default()
        },
    );
    assert_eq!(simulation.pose(), source);
    for open in [true, false] {
        simulation.set_open(open);
        let mut connected = false;
        let mut separate = false;
        for _ in 0..360 {
            simulation.tick();
            let contour = trace(&simulation).unwrap();
            connected |= contour.loops.len() == 1;
            separate |= contour.loops.len() == 2;
        }
        assert!(connected && separate, "missing separation/fusion: {open}");
        simulation.finish();
        assert_eq!(
            trace(&simulation).unwrap().loops.len(),
            if open { 2 } else { 1 }
        );
    }
    assert_eq!(simulation.pose().w, source.w);
    assert_eq!(simulation.pose().h, source.h);
}

#[test]
fn layout_translation_preserves_live_shapes_and_future_motion() {
    let poses = [
        Pose::rect(0., 40., 600., 80., 32.),
        Pose::rect(24., 8., 120., 32., 16.),
    ];
    let mut original = Simulation::compound(&poses, 4., Material::default(), Options::default());
    original.finish();
    original.set_compound_targets(&[
        Pose::rect(0., 20., 520., 100., 32.),
        Pose::rect(64., -12., 160., 32., 16.),
    ]);
    original.advance(0.05, false);
    let mut shifted = original.clone();
    let moving = shifted.moving();
    let mass = shifted.mass();
    shifted.translate([-106., 24.]);
    assert_eq!(moving, shifted.moving());
    assert_eq!(mass, shifted.mass());
    for _ in 0..30 {
        for index in 0..2 {
            let a = original.group_pose(index);
            let b = shifted.group_pose(index);
            assert!((b.cx - a.cx + 106.).abs() < 1e-7);
            assert!((b.cy - a.cy - 24.).abs() < 1e-7);
            for (a, b) in [
                (a.w, b.w),
                (a.h, b.h),
                (a.vx, b.vx),
                (a.vy, b.vy),
                (a.vw, b.vw),
                (a.vh, b.vh),
            ] {
                assert!((a - b).abs() < 1e-7);
            }
        }
        original.advance(1. / 60., false);
        shifted.advance(1. / 60., false);
    }
}
