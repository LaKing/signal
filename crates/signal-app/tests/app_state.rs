//! M2.5-1 AC3: the app's state without a window, through the null driver:
//! open a project, play, meter blocks from the runner and from a simulated
//! input, stop; and the Setup's devices, channel map and window size.

use std::fs;
use std::path::{Path, PathBuf};

use signal_app::dialog::{Choice, Draft};
use signal_app::setup::{DeviceEntry, Setup, Window};
use signal_app::state::{AppState, Backend, Command, Interface, Selection, View};
use signal_audio::{ConfigRange, DeviceInfo, DeviceRef, Direction, Resolution};

fn device(name: &str, id: &str, direction: Direction, min: u32, max: u32) -> DeviceInfo {
    DeviceInfo {
        name: name.to_owned(),
        id: Some(id.to_owned()),
        direction,
        default: false,
        configs: vec![ConfigRange {
            channels: 2,
            min_rate: min,
            max_rate: max,
            buffer: Some((16, 4096)),
            sample_format: "f32".to_owned(),
        }],
    }
}

/// Two same-named outputs, a third output and an input.
fn devices() -> Vec<DeviceInfo> {
    vec![
        device(
            "DELL U3219Q",
            "null:dell-a",
            Direction::Output,
            48_000,
            48_000,
        ),
        device(
            "DELL U3219Q",
            "null:dell-b",
            Direction::Output,
            48_000,
            48_000,
        ),
        device("Null Out", "null:out", Direction::Output, 44_100, 96_000),
        device("Null In", "null:in", Direction::Input, 44_100, 96_000),
    ]
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn kick() -> PathBuf {
    root().join("projects/kick")
}

fn tone() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone")
}

/// A Setup file of its own for each test.
fn setup_file(name: &str, setup: &Setup) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("signal-app-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("setup.toml");
    setup.save(&path).unwrap();
    path
}

fn entry(index: usize) -> DeviceEntry {
    DeviceEntry::from_ref(DeviceRef::of(&devices()[index]))
}

fn state(name: &str, setup: &Setup, project: &Path) -> AppState {
    let path = setup_file(name, setup);
    AppState::new(path, Some(project.to_owned()), Backend::Null(devices()))
}

fn play(state: &mut AppState) -> View {
    state.apply(Command::Play);
    let view = state.view();
    assert!(view.playing, "refused: {:?}", view.message);
    view
}

#[test]
fn open_play_meter_and_stop_through_the_null_driver() {
    let setup = Setup {
        output: Some(entry(2)),
        input: Some(entry(3)),
        ..Setup::default()
    };
    let mut state = state("play", &setup, &kick());
    let view = state.view();
    assert_eq!(view.sample_rate, Some(96_000));
    assert!(view.diagnostics.is_empty());
    assert_eq!(view.output, Some(Resolution::Found(2)));
    assert!(view.outputs.is_empty(), "no meters before play");

    let view = play(&mut state);
    let labels: Vec<&str> = view.outputs.iter().map(|m| m.label.as_str()).collect();
    assert_eq!(labels, ["out 1 → 1", "out 2 → 2"]);
    assert_eq!(view.inputs.len(), 2);

    // Meter blocks from the runner.
    state.run_null(48_000).unwrap();
    state.tick();
    let view = state.view();
    assert_eq!(view.status.unwrap().frame, 48_000);
    assert!((view.status.unwrap().elapsed - 0.5).abs() < 1e-12);
    // M2.6-3: the worker count and the scheduling path (auto threads; the
    // null backend leaves the workers ordinary).
    let status = view.status.unwrap();
    assert_eq!(status.threads, signal_engine::realtime_threads());
    assert_eq!(status.workers, status.threads - 1);
    assert_eq!(status.scheduling, "none");
    for meter in &view.outputs {
        assert!(
            meter.reading.held.is_finite(),
            "{meter:?}: the kick is metered"
        );
    }
    // Simulated input blocks: silence, then a full-scale left channel.
    for meter in &view.inputs {
        assert_eq!(meter.reading.rms, f64::NEG_INFINITY);
    }
    state.push_input(&[0.0; 256]).unwrap();
    state.push_input(&[1.0, 0.25].repeat(128)).unwrap();
    state.tick();
    let inputs = state.view().inputs;
    assert_eq!(inputs[0].reading.peak, 0.0);
    assert!(inputs[0].reading.clip, "full scale latches");
    assert!((inputs[1].reading.peak - 20.0 * 0.25f64.log10()).abs() < 1e-9);
    assert!(!inputs[1].reading.clip);
    state.apply(Command::ClearClip {
        input: true,
        channel: 0,
    });
    assert!(!state.view().inputs[0].reading.clip);

    state.apply(Command::Stop);
    let view = state.view();
    assert!(!view.playing);
    assert_eq!(
        view.status.unwrap().frame,
        48_000,
        "the last run's status stays"
    );
    assert!(state.run_null(1).is_err(), "the driver is gone");
}

#[test]
fn a_saved_device_that_is_gone_is_not_replaced_by_its_twin() {
    // DELL a was saved; the list now holds only DELL b of that name.
    let setup = Setup {
        output: Some(DeviceEntry {
            id: Some("null:dell-gone".to_owned()),
            ..entry(0)
        }),
        ..Setup::default()
    };
    let mut state = state("gone", &setup, &tone());
    assert!(matches!(
        state.view().output,
        Some(Resolution::Missing { .. })
    ));
    state.apply(Command::Play);
    let view = state.view();
    assert!(!view.playing);
    assert_eq!(
        view.message.as_deref(),
        Some("the saved output 'DELL U3219Q' (id null:dell-gone) is not present; choose a device")
    );
    // Without an id, two same-named devices are ambiguous.
    let setup = Setup {
        output: Some(DeviceEntry {
            id: None,
            ..entry(0)
        }),
        ..Setup::default()
    };
    let mut state = self::state("ambiguous", &setup, &tone());
    state.apply(Command::Play);
    assert!(!state.view().playing);
    assert!(
        state
            .view()
            .message
            .unwrap()
            .contains("no id to tell them apart")
    );
}

#[test]
fn a_channel_map_that_does_not_fit_plays_on_the_identity_map() {
    let setup = Setup {
        output_channels: Some(vec![2]),
        ..Setup::default()
    };
    let mut state = state("map", &setup, &tone());
    let view = play(&mut state);
    assert_eq!(
        view.notes,
        [
            "output channel map [2]: the channel map: 1 entries, but the sink has 2 channels; playing on the identity map"
        ]
    );
    let labels: Vec<&str> = view.outputs.iter().map(|m| m.label.as_str()).collect();
    assert_eq!(labels, ["out 1 → 1", "out 2 → 2"]);
}

#[test]
fn devices_are_listed_again_only_while_stopped() {
    let mut state = state("refresh", &Setup::default(), &tone());
    let before = state.view().devices;
    assert_eq!(before.outputs.len(), 3);
    assert!(before.outputs[0].1.starts_with("DELL U3219Q (#1 · "));
    play(&mut state);
    state.apply(Command::RefreshDevices);
    let view = state.view();
    assert_eq!(
        view.message.as_deref(),
        Some("stop playback to refresh: listing devices pauses metering and status")
    );
    assert_eq!(view.devices, before);
    state.apply(Command::Stop);
    state.apply(Command::RefreshDevices);
    assert_eq!(state.view().message, None);
}

#[test]
fn apply_saves_and_restarts_from_frame_0() {
    let path = setup_file("apply", &Setup::default());
    let mut state = AppState::new(path.clone(), Some(tone()), Backend::Null(devices()));
    play(&mut state);
    state.run_null(4_800).unwrap();
    state.tick();
    assert_eq!(state.view().status.unwrap().frame, 4_800);
    state.apply(Command::Apply(Box::new(Interface {
        output: Selection::Device(DeviceRef::of(&devices()[1])),
        input: Selection::Unset,
        buffer: 256,
        cpu_workers: Some(2),
        output_channels: None,
    })));
    let view = state.view();
    assert!(view.playing);
    assert_eq!(
        view.message.as_deref(),
        Some("applied; playing again from frame 0")
    );
    let status = view.status.unwrap();
    assert_eq!((status.frame, status.buffer), (0, 256));
    let (saved, problems) = Setup::load(&path);
    assert!(problems.is_empty());
    assert_eq!(saved.output.unwrap().id.as_deref(), Some("null:dell-b"));
    assert_eq!(saved.buffer, 256);
    assert_eq!(saved.cpu_workers, Some(2));
    assert_eq!(saved.last_project, Some(tone()));
}

#[test]
fn exit_saves_the_window_size() {
    let path = setup_file("exit", &Setup::default());
    let mut state = AppState::new(path.clone(), Some(tone()), Backend::Null(devices()));
    play(&mut state);
    let window = Window {
        width: 1000.0,
        height: 700.0,
    };
    state.apply(Command::Exit(Some(window)));
    assert!(!state.is_playing());
    assert_eq!(Setup::load(&path).0.window, Some(window));
}

/// M2.5-3: started without `--project` (as Signal.app is from the Finder),
/// the app opens the Setup's last project.
#[test]
fn a_start_without_a_project_opens_the_last_one() {
    let setup = Setup {
        last_project: Some(tone()),
        ..Setup::default()
    };
    let path = setup_file("last", &setup);
    let state = AppState::new(path, None, Backend::Null(devices()));
    let view = state.view();
    assert_eq!(view.project, tone().display().to_string());
    assert_eq!(view.sample_rate, Some(48_000));
    assert!(view.diagnostics.is_empty());
}

#[test]
fn a_project_that_does_not_load_shows_its_diagnostic() {
    let dir = std::env::temp_dir().join(format!("signal-app-bad-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("project.toml"), "schema_version = [").unwrap();
    let mut state = state("bad", &Setup::default(), &dir);
    let view = state.view();
    assert_eq!(view.sample_rate, None);
    assert!(view.diagnostics[0].starts_with(dir.join("project.toml").to_str().unwrap()));
    state.apply(Command::Play);
    assert!(!state.view().playing);
    fs::remove_dir_all(&dir).unwrap();
}

/// A saved output that is not present stays saved when Apply is pressed
/// without choosing another; playing stays refused until a device (here the
/// system default) is chosen explicitly.
#[test]
fn a_missing_output_is_kept_until_another_is_chosen() {
    let gone = DeviceEntry {
        id: Some("null:dell-gone".to_owned()),
        ..entry(0)
    };
    let path = setup_file(
        "keep",
        &Setup {
            output: Some(gone.clone()),
            ..Setup::default()
        },
    );
    let mut state = AppState::new(path.clone(), Some(tone()), Backend::Null(devices()));
    let mut draft = Draft::new(&state.view());
    assert!(
        matches!(draft.output, Choice::Unresolved(_)),
        "{:?}",
        draft.output
    );
    state.apply(Command::Apply(Box::new(draft.interface().unwrap())));
    assert_eq!(Setup::load(&path).0.output, Some(gone));
    state.apply(Command::Play);
    assert!(!state.view().playing, "still refused");

    draft.select(Direction::Output, None, &state.view().devices.devices);
    state.apply(Command::Apply(Box::new(draft.interface().unwrap())));
    assert_eq!(Setup::load(&path).0.output, None);
    play(&mut state);
}

/// Map [3, 4] on a device offering 2- and 4-channel configurations plays on
/// device channels 3 and 4, not on the identity map.
#[test]
fn a_map_to_channels_3_and_4_takes_the_wider_configuration() {
    let range = |channels| ConfigRange {
        channels,
        min_rate: 48_000,
        max_rate: 48_000,
        buffer: Some((16, 4096)),
        sample_format: "f32".to_owned(),
    };
    let quad = DeviceInfo {
        configs: vec![range(2), range(4)],
        ..device("Quad Out", "null:quad", Direction::Output, 48_000, 48_000)
    };
    let setup = Setup {
        output: Some(DeviceEntry::from_ref(DeviceRef::of(&quad))),
        output_channels: Some(vec![3, 4]),
        ..Setup::default()
    };
    let path = setup_file("quad", &setup);
    let mut state = AppState::new(path, Some(tone()), Backend::Null(vec![quad]));
    let view = play(&mut state);
    assert!(view.notes.is_empty(), "{:?}", view.notes);
    let labels: Vec<&str> = view.outputs.iter().map(|m| m.label.as_str()).collect();
    assert_eq!(labels, ["out 1 → 3", "out 2 → 4"]);
}
