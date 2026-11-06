//! ADR 0038 §4, §7: the input devices a Signal.app run captures from. A
//! capture role bound to a device that is not present reads 0: it never
//! falls back to main's input (an unplugged mic must not become the room
//! mic on the speakers). The null driver captures nothing.

use signal_audio::{ConfigRange, DeviceInfo, Direction};
use signal_output::CaptureSide;
use signal_ui::player::{Backend, capture_specs};
use signal_ui::setup::{DeviceEntry, PlayEntry, Setup};

fn input(name: &str, id: &str, default: bool) -> DeviceInfo {
    DeviceInfo {
        name: name.to_owned(),
        id: Some(id.to_owned()),
        direction: Direction::Input,
        default,
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

fn voice() -> Vec<CaptureSide> {
    vec![CaptureSide {
        address: "/mic".parse().unwrap(),
        role: "voice".to_owned(),
        width: 1,
    }]
}

fn bound_to_at2020() -> Setup {
    let mut setup = Setup::default();
    setup.capture.insert(
        "voice".to_owned(),
        PlayEntry {
            channels: vec![1],
            device: Some(DeviceEntry {
                name: "AT2020USB+".to_owned(),
                id: Some("coreaudio:at".to_owned()),
                signature: String::new(),
            }),
        },
    );
    setup
}

#[test]
fn a_bound_input_is_captured_from_its_device() {
    let devices = [
        input("SSL 2+ Mk II", "coreaudio:ssl", true),
        input("AT2020USB+", "coreaudio:at", false),
    ];
    let mut notes = Vec::new();
    let specs = capture_specs(
        &Backend::CoreAudio,
        &devices,
        &bound_to_at2020(),
        &voice(),
        &mut notes,
    );
    assert_eq!(specs.len(), 1, "{notes:?}");
    assert_eq!(specs[0].name, "AT2020USB+");
    assert_eq!(specs[0].sides[0].channels, [Some(0)]);
}

#[test]
fn an_unplugged_input_reads_0_and_never_falls_back_to_main() {
    let devices = [input("SSL 2+ Mk II", "coreaudio:ssl", true)];
    let mut notes = Vec::new();
    let specs = capture_specs(
        &Backend::CoreAudio,
        &devices,
        &bound_to_at2020(),
        &voice(),
        &mut notes,
    );
    assert!(specs.is_empty(), "{specs:?}");
    assert!(notes.iter().any(|n| n.contains("AT2020USB+")), "{notes:?}");
    assert!(
        !notes.iter().any(|n| n.contains("captures main's input")),
        "{notes:?}"
    );
}

#[test]
fn the_null_driver_captures_nothing() {
    let devices = [input("AT2020USB+", "coreaudio:at", true)];
    let mut notes = Vec::new();
    let specs = capture_specs(
        &Backend::Null(Vec::new()),
        &devices,
        &bound_to_at2020(),
        &voice(),
        &mut notes,
    );
    assert!(specs.is_empty());
    assert!(notes[0].contains("null driver"));
}
