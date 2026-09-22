use super::*;

fn command(id: u32, kind: u32, flags: u32, x: f32) -> Vec<u8> {
    let mut words = [0_u32; 24];
    words[0] = id;
    words[1] = kind;
    words[2] = flags;
    for (i, value) in [
        (4, x),
        (5, 0.),
        (6, 96.),
        (7, 40.),
        (8, 20.),
        (9, x),
        (10, 60.),
        (11, 280.),
        (12, 160.),
        (13, 32.),
        (14, 0.5),
        (15, 0.5),
        (18, 0.6),
        (19, 0.5),
    ] {
        words[i] = f32::to_bits(value);
    }
    words[16] = 3;
    words.into_iter().flat_map(u32::to_ne_bytes).collect()
}
fn at(bytes: &[u8], i: usize) -> u32 {
    u32::from_ne_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
}

#[test]
fn static_and_idle_frames_do_no_physics_or_geometry_work() {
    let mut scene = Scene::default();
    let first = scene.frame(&command(1, 0, 1, 0.), 0., false).unwrap();
    assert_eq!((at(first, 1), at(first, 2), at(first, 3)), (1, 0, 0));
    assert!(scene.nodes[&1].surface.is_none());
    let first = scene.nodes[&1].static_contour.as_ref().unwrap().clone();
    for _ in 0..100 {
        let idle = scene.frame(&[], 1. / 120., false).unwrap();
        assert_eq!(idle.len(), 16);
        assert_eq!((at(idle, 1), at(idle, 2)), (0, 0));
        assert!(Rc::ptr_eq(
            &first,
            scene.nodes[&1].static_contour.as_ref().unwrap()
        ));
    }
}

#[test]
fn only_placement_is_transferred_when_a_static_path_moves() {
    let mut scene = Scene::default();
    scene.frame(&command(1, 0, 1, 0.), 0., false).unwrap();
    let moved = scene.frame(&command(1, 0, 1, 80.), 0., false).unwrap();
    assert_eq!(at(moved, 5), 0, "no new path payload");
    assert_eq!(moved.len(), 16 + 64);
    assert_eq!(f32::from_bits(at(moved, 11)), 80.);
}

#[test]
fn a_tap_received_between_display_frames_is_visible_then_settles() {
    let mut scene = Scene::default();
    scene.frame(&command(1, 1, 1, 0.), 0., false).unwrap();
    assert!(
        scene.nodes[&1].surface.is_none(),
        "idle buttons do not allocate a solver"
    );
    scene.frame(&command(1, 1, 1 | 8, 0.), 0., false).unwrap();
    let mut lowest = 40_f64;
    let mut stopped = false;
    for _ in 0..400 {
        let bytes = scene.frame(&[], 1. / 120., false).unwrap();
        assert_eq!(at(bytes, 3), 0);
        let moving = at(bytes, 2);
        lowest = lowest.min(
            scene.nodes[&1]
                .surface
                .as_ref()
                .unwrap()
                .simulation
                .pose()
                .h,
        );
        if moving == 0 {
            stopped = true;
            break;
        }
    }
    assert!(
        lowest >= 38.5,
        "ordinary press keeps rest height (no liquid squash)"
    );
    assert!(stopped);
    assert_eq!(scene.frame(&[], 1., false).unwrap().len(), 16);
}

#[test]
fn hidden_and_reduced_scenes_finish_without_background_catch_up() {
    for flags in [1, 0] {
        let mut scene = Scene::default();
        scene.frame(&command(1, 5, 1, 0.), 0., false).unwrap();
        let bytes = scene
            .frame(&command(1, 5, flags | 2, 0.), 0., flags == 1)
            .unwrap();
        assert_eq!((at(bytes, 2), at(bytes, 3)), (0, 0));
        assert_eq!(scene.nodes[&1].transition.progress(), 1.);
        assert!(!scene.nodes[&1]
            .surface
            .as_ref()
            .unwrap()
            .simulation
            .moving());
        assert_eq!(scene.frame(&[], 30., false).unwrap().len(), 16);
    }
}

#[test]
fn invalid_batches_do_not_change_existing_nodes() {
    let mut scene = Scene::default();
    scene.frame(&command(1, 0, 1, 0.), 0., false).unwrap();
    let prior = scene.nodes[&1].update.from;
    let mut batch = command(1, 0, 1, 100.);
    let mut invalid = command(2, 1, 1, 0.);
    invalid[4 * 8..4 * 9].copy_from_slice(&f32::NAN.to_ne_bytes());
    batch.extend(invalid);
    assert_eq!(scene.frame(&batch, 0., false).unwrap_err(), Error::Command);
    assert_eq!(scene.nodes[&1].update.from, prior);
    assert_eq!(scene.len(), 1);
    assert_eq!(scene.frame(&[0], 0., false).unwrap_err(), Error::Command);
}

#[test]
fn duplicate_updates_do_not_consume_extra_capacity_and_removal_releases_nodes() {
    let mut scene = Scene::default();
    let batch = (0..300)
        .flat_map(|_| command(1, 0, 1, 0.))
        .collect::<Vec<_>>();
    scene.frame(&batch, 0., false).unwrap();
    assert_eq!(scene.len(), 1);
    scene.frame(&command(1, 7, 0, 0.), 0., false).unwrap();
    assert!(scene.is_empty());
}

fn member(id: u32, parent: u32, x: f32) -> Vec<u8> {
    let mut update = command(id, 10, 1, x);
    update[12..16].copy_from_slice(&parent.to_ne_bytes());
    update
}

#[test]
fn compound_members_keep_identity_and_velocity_through_retarget_and_removal() {
    let mut scene = Scene::default();
    let mut input = command(1, 9, 1, 0.);
    input.extend(member(11, 1, 4.));
    input.extend(member(12, 1, 24.));
    let frame = scene.frame(&input, 1. / 120., false).unwrap();
    assert_eq!(at(frame, 1), 3);
    assert_eq!(at(frame, 3), 0);
    for _ in 0..6 {
        scene.frame(&[], 1. / 120., false).unwrap();
    }
    let before = scene.nodes[&1]
        .surface
        .as_ref()
        .unwrap()
        .simulation
        .group_pose(2);
    scene.frame(&member(12, 1, 100.), 0., false).unwrap();
    assert_eq!(
        scene.nodes[&1]
            .surface
            .as_ref()
            .unwrap()
            .simulation
            .group_pose(2),
        before
    );
    scene.frame(&command(11, 7, 0, 0.), 0., false).unwrap();
    assert_eq!(scene.nodes[&1].members, [12]);
    assert_eq!(
        scene.nodes[&1]
            .surface
            .as_ref()
            .unwrap()
            .simulation
            .group_pose(1),
        before
    );
    for _ in 0..800 {
        scene.frame(&[], 1. / 120., false).unwrap();
    }
    assert_eq!(scene.frame(&[], 1., false).unwrap().len(), 16);
    assert_eq!(scene.len(), 2);
    scene.frame(&command(1, 7, 0, 0.), 0., false).unwrap();
    assert!(scene.is_empty());
    assert_eq!(scene.len(), 0);
}

#[test]
fn compound_validation_and_reduced_motion_preserve_the_protocol() {
    let mut scene = Scene::default();
    assert_eq!(
        scene.frame(&member(11, 1, 0.), 0., false).unwrap_err(),
        Error::Command
    );
    assert!(scene.is_empty());
    let mut input = member(11, 1, 0.);
    input.extend(command(1, 9, 1, 0.));
    let frame = scene.frame(&input, 0., true).unwrap();
    assert_eq!((at(frame, 1), at(frame, 2), at(frame, 3)), (2, 0, 0));
    assert_eq!(scene.frame(&[], 1., false).unwrap().len(), 16);
    let mut collision = member(21, 1, 0.);
    collision.extend(command(21, 0, 1, 0.));
    assert_eq!(
        scene.frame(&collision, 0., false).unwrap_err(),
        Error::Command
    );
    assert_eq!(scene.len(), 2);
}
