//! ADR 0038 §4, §5: a Setup's `main` map of another length than the main
//! side is the machine's layout, not a map the device cannot take: a 5.1
//! `main` on `output_channels = [1, 2]` folds to stereo there, and the
//! author's stereo version of it plays as it is.

use signal_audio::{ConfigRange, DeviceInfo, Direction};
use signal_output::{Heard, PlaySide};
use signal_ui::player::{Backend, open_routed};
use signal_ui::setup::Setup;

fn speakers() -> DeviceInfo {
    DeviceInfo {
        name: "Speakers".to_owned(),
        id: Some("coreaudio:speakers".to_owned()),
        direction: Direction::Output,
        default: true,
        rate: Some(48_000),
        configs: vec![ConfigRange {
            channels: 2,
            min_rate: 44_100,
            max_rate: 48_000,
            buffer: Some((15, 4096)),
            sample_format: "f32".to_owned(),
        }],
    }
}

fn side(address: &str, width: usize) -> PlaySide {
    PlaySide {
        address: address.parse().unwrap(),
        role: "main".to_owned(),
        width,
        ports: Vec::new(),
    }
}

#[test]
fn a_stereo_map_folds_a_surround_main_or_plays_its_author_s_version() {
    let devices = vec![speakers()];
    let backend = Backend::Null(devices.clone());
    let setup = Setup {
        output_channels: Some(vec![1, 2]),
        ..Setup::default()
    };
    let mut notes = Vec::new();
    let (_, routing) = open_routed(
        &backend,
        &devices,
        &setup,
        48_000,
        &[side("/room", 6)],
        1,
        &mut notes,
    )
    .unwrap();
    assert_eq!(routing.width(), 2);
    assert_eq!(routing.targets(), [Some(0), Some(1)]);
    assert!(
        notes
            .iter()
            .any(|n| n.ends_with("5.1 → stereo (ITU, LFE off)")),
        "{notes:?}"
    );
    assert!(
        !notes.iter().any(|n| n.contains("cannot take")),
        "{notes:?}"
    );

    let mut notes = Vec::new();
    let (_, routing) = open_routed(
        &backend,
        &devices,
        &setup,
        48_000,
        &[side("/room", 6), side("/fold", 2)],
        1,
        &mut notes,
    )
    .unwrap();
    let fold = routing.side(&"/fold".parse().unwrap()).unwrap();
    assert_eq!(fold.channels, [Some(1), Some(2)]);
    assert_eq!(
        routing.side(&"/room".parse().unwrap()).unwrap().heard,
        Heard::Version("/fold".parse().unwrap())
    );

    // A map the device cannot take plays on the default, reported.
    let bad = Setup {
        output_channels: Some(vec![3, 4]),
        ..Setup::default()
    };
    let mut notes = Vec::new();
    let (_, routing) = open_routed(
        &backend,
        &devices,
        &bad,
        48_000,
        &[side("/room", 6)],
        1,
        &mut notes,
    )
    .unwrap();
    assert_eq!(routing.targets(), [Some(0), Some(1)]);
    assert!(notes.iter().any(|n| n.contains("cannot take")), "{notes:?}");
}
