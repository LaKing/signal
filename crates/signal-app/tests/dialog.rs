//! The interface dialog's draft without a window: a chosen device keeps its
//! identity across refreshes, and the system default output shows its
//! capabilities and the rate check.

use signal_app::dialog::{Choice, Draft, capabilities, rate_check};
use signal_app::setup::Setup;
use signal_app::state::{DeviceList, Selection, View};
use signal_audio::{ConfigRange, DeviceInfo, DeviceRef, Direction};

fn device(id: &str, default: bool, rate: u32) -> DeviceInfo {
    DeviceInfo {
        name: "DELL U3219Q".to_owned(),
        id: Some(id.to_owned()),
        direction: Direction::Output,
        default,
        configs: vec![ConfigRange {
            channels: 2,
            min_rate: rate,
            max_rate: rate,
            buffer: Some((16, 4096)),
            sample_format: "f32".to_owned(),
        }],
    }
}

fn view(devices: Vec<DeviceInfo>) -> View {
    View {
        setup: Some(Setup::default()),
        devices: DeviceList {
            devices,
            ..DeviceList::default()
        },
        ..View::default()
    }
}

/// Two same-named, same-signature devices A and B: A is chosen, the list is
/// refreshed in the other order, and A is still what Apply sends.
#[test]
fn a_chosen_device_keeps_its_identity_when_the_list_is_reordered() {
    let (a, b) = (device("a", false, 48_000), device("b", false, 48_000));
    let mut draft = Draft::new(&view(vec![a.clone(), b.clone()]));
    draft.select(Direction::Output, Some(0), &[a.clone(), b.clone()]);
    let reordered = [b, a.clone()];
    draft.refreshed(&reordered);
    assert_eq!(draft.index(Direction::Output, &reordered), Some(1));
    assert_eq!(
        draft.interface().unwrap().output,
        Selection::Device(DeviceRef::of(&a))
    );
}

/// A chosen device that is gone after a refresh is unresolved, never its
/// twin, and Apply keeps the Setup's device.
#[test]
fn a_chosen_device_that_disappears_becomes_unresolved() {
    let (a, b) = (device("a", false, 48_000), device("b", false, 48_000));
    let mut draft = Draft::new(&view(vec![a.clone(), b.clone()]));
    draft.select(Direction::Output, Some(0), &[a, b.clone()]);
    draft.refreshed(std::slice::from_ref(&b));
    assert_eq!(
        draft.output,
        Choice::Unresolved(
            "the saved output 'DELL U3219Q' (id a) is not present; choose a device".to_owned()
        )
    );
    assert_eq!(draft.index(Direction::Output, &[b]), None);
    assert_eq!(draft.interface().unwrap().output, Selection::Keep);
}

/// With the system default output chosen, the dialog shows the default
/// device's capabilities and says when the project's rate is not among them.
#[test]
fn the_default_output_shows_its_capabilities_and_the_rate_refusal() {
    let devices = vec![
        device("other", false, 96_000),
        device("default", true, 48_000),
    ];
    let draft = Draft::new(&view(devices.clone()));
    assert_eq!(draft.output, Choice::Unset);
    let shown = draft.output_device(&devices).unwrap();
    assert_eq!(shown.id.as_deref(), Some("default"));
    assert_eq!(
        capabilities(shown),
        ["2 channels, 48000 Hz, f32, buffer 16-4096 frames"]
    );
    assert_eq!(
        rate_check(shown, 96_000, 128),
        Err(
            "the device 'DELL U3219Q' cannot run at 96000 Hz; it supports 48000 Hz. \
             Signal does not resample: set sample_rate in project.toml to one of these"
                .to_owned()
        )
    );
    assert_eq!(
        rate_check(shown, 48_000, 128),
        Ok("The project's 48000 Hz is supported.".to_owned())
    );
}
