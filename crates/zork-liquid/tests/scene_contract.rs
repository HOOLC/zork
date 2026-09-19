//! Consumer-side checks of the process-local scene protocol, using public APIs.
use std::collections::BTreeMap;
use zork_liquid::scene::{Scene, COMMAND_BYTES, VERSION};

fn command(id: u32, kind: u32, flags: u32) -> Vec<u8> {
    let mut words = [0_u32; 24];
    words[..4].copy_from_slice(&[id, kind, flags, 0]);
    for (offset, value) in [
        (4, 20_f32),
        (5, 40.),
        (6, 96.),
        (7, 32.),
        (8, 16.),
        (9, 120.),
        (10, 180.),
        (11, 280.),
        (12, 240.),
        (13, 32.),
        (14, 0.5),
        (15, 0.5),
        (18, 0.6),
        (19, 0.5),
    ] {
        words[offset] = value.to_bits();
    }
    words[16] = 3;
    let bytes: Vec<_> = words.into_iter().flat_map(u32::to_ne_bytes).collect();
    assert_eq!(bytes.len(), COMMAND_BYTES);
    bytes
}

fn word(bytes: &[u8], index: usize) -> u32 {
    u32::from_ne_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
}

#[derive(Debug)]
struct Record {
    flags: u32,
    length: usize,
    content: f32,
    backdrop: f32,
    expansion: f32,
    alive: bool,
}

fn decode(bytes: &[u8]) -> (u32, BTreeMap<u32, Record>) {
    assert_eq!(word(bytes, 0), 3);
    assert_eq!(word(bytes, 3), 0, "scene reported an encoding error");
    let mut offset = 16;
    let mut records = BTreeMap::new();
    for _ in 0..word(bytes, 1) {
        let record = &bytes[offset..];
        let length = word(record, 2) as usize;
        let header = 64;
        assert!(length >= header && offset + length <= bytes.len());
        let mut cursor = header;
        // Match the platform's path decoder, including the slider's two paths.
        for kind in 3..7 {
            for _ in 0..word(record, kind) {
                let count = word(record, cursor / 4) as usize;
                cursor += 4;
                let coordinates = if kind == 4 { count * 2 } else { 2 + count * 6 };
                for _ in 0..coordinates {
                    assert!(f32::from_bits(word(record, cursor / 4)).is_finite());
                    cursor += 4;
                }
            }
        }
        assert_eq!(cursor, length, "record length/path layout changed");
        let scalar = |index| f32::from_bits(word(record, index));
        let values = [scalar(12), scalar(13), scalar(14)];
        assert!(values.into_iter().all(|value| (0. ..=1.).contains(&value)));
        let state = word(record, 15);
        assert_eq!(state & !1, 0);
        assert!(records
            .insert(
                word(record, 0),
                Record {
                    flags: word(record, 1),
                    length,
                    content: values[0],
                    backdrop: values[1],
                    expansion: values[2],
                    alive: state & 1 != 0,
                }
            )
            .is_none());
        offset += length;
    }
    assert_eq!(offset, bytes.len());
    (word(bytes, 2), records)
}

#[test]
fn v3_records_cover_all_path_kinds_and_geometry_resends() {
    assert_eq!(VERSION, 3);
    let mut scene = Scene::default();
    let mut input = command(1, 0, 1);
    input.extend(command(2, 8, 1));
    input.extend(command(3, 9, 1));
    let mut member = command(4, 10, 1);
    member[12..16].copy_from_slice(&3_u32.to_ne_bytes());
    input.extend(member);
    let (_, records) = decode(scene.frame(&input, 0., true).unwrap());
    assert_eq!(records.len(), 4);
    assert_eq!(records[&4].length, 64);
    assert!(records
        .values()
        .all(|r| !r.alive && r.content == 0. && r.backdrop == 0.));

    let mut moved = command(1, 0, 1);
    moved[16..20].copy_from_slice(&80_f32.to_ne_bytes());
    let (_, records) = decode(scene.frame(&moved, 0., true).unwrap());
    assert_eq!(records[&1].flags & 1, 0, "translation rebuilt a path");
    assert_eq!(records[&1].length, 64);
    let (_, records) = decode(scene.frame(&command(1, 0, 1 | 16), 0., true).unwrap());
    assert_ne!(
        records[&1].flags & 1,
        0,
        "explicit geometry resend was ignored"
    );
}

#[test]
fn reveals_wait_for_expansion_and_survive_retargeting_and_reversal() {
    for kind in [5, 6] {
        let mut scene = Scene::default();
        let (_, first) = decode(scene.frame(&command(1, kind, 3), 0., false).unwrap());
        assert!(first[&1].alive);
        assert_eq!((first[&1].content, first[&1].backdrop), (0., 0.));
        let mut entered = false;
        let mut independent = false;
        let mut settled = false;
        for _ in 0..1600 {
            let (moving, records) = decode(scene.frame(&[], 1. / 240., false).unwrap());
            if let Some(record) = records.get(&1) {
                entered |= record.expansion >= 0.5;
                if !entered {
                    assert_eq!(record.backdrop, 0.);
                }
                independent |= record.content > 0. && record.backdrop == 0.;
                assert!(record.alive);
                if moving == 0 {
                    assert_eq!((record.content, record.backdrop), (1., 1.));
                }
            }
            if moving == 0 {
                settled = true;
                break;
            }
        }
        assert!(entered && independent && settled);
        let mut retarget = command(1, kind, 3);
        retarget[48..52].copy_from_slice(&400_f32.to_ne_bytes());
        let (_, resized) = decode(scene.frame(&retarget, 0., false).unwrap());
        assert_eq!(
            resized[&1].backdrop, 1.,
            "remeasurement restarted the scrim"
        );
        let (_, closed) = decode(scene.frame(&command(1, kind, 1), 1. / 120., false).unwrap());
        assert!(closed[&1].alive && closed[&1].backdrop < 1.);
        let (_, reversed) = decode(scene.frame(&command(1, kind, 3), 0., false).unwrap());
        assert_eq!(reversed[&1].content, closed[&1].content);
        assert_eq!(reversed[&1].backdrop, closed[&1].backdrop);

        scene.frame(&command(1, kind, 1), 0., false).unwrap();
        let mut stopped = false;
        for _ in 0..1600 {
            let (moving, records) = decode(scene.frame(&[], 1. / 240., false).unwrap());
            if moving == 0 {
                let record = &records[&1];
                assert!(!record.alive);
                assert_eq!((record.content, record.backdrop), (0., 0.));
                stopped = true;
                break;
            }
        }
        assert!(stopped, "exit never released its presentation lifetime");
        assert_eq!(scene.frame(&[], 1., false).unwrap().len(), 16);
    }
}

#[test]
fn hidden_reduced_and_retry_frames_have_unambiguous_terminal_state() {
    let mut scene = Scene::default();
    let (_, opened) = decode(scene.frame(&command(1, 6, 3), 0., true).unwrap());
    assert_eq!((opened[&1].content, opened[&1].backdrop), (1., 1.));
    assert!(opened[&1].alive);
    let saved = scene.pending().to_vec();
    assert_eq!(
        scene.pending(),
        saved,
        "buffer retry must not advance animation"
    );
    assert!(scene.frame(&[0], 0., false).is_err());
    assert_eq!(
        scene.pending(),
        saved,
        "rejected batch replaced the valid result"
    );
    let (moving, hidden) = decode(scene.frame(&command(1, 6, 2), 0., false).unwrap());
    assert_eq!(moving, 0);
    assert_eq!(hidden[&1].flags, 8);
    assert!(!hidden[&1].alive);
    assert_eq!((hidden[&1].content, hidden[&1].backdrop), (0., 0.));
    assert_eq!(hidden[&1].length, 64);
    scene.frame(&command(1, 6, 3), 0., true).unwrap();
    let (moving, closed) = decode(scene.frame(&command(1, 6, 1), 0., true).unwrap());
    assert_eq!(moving, 0);
    assert!(!closed[&1].alive);
    assert_eq!((closed[&1].content, closed[&1].backdrop), (0., 0.));
    scene.frame(&command(1, 7, 0), 0., false).unwrap();
    assert!(scene.is_empty());
}
