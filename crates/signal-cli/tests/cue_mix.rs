//! `projects/cue-mix` (ADR 0038, M4.1): two outputs at once, `main`
//! (`std.audio_out`) and `cue` (a stereo device slot of role `cue`).
//!
//! - `signal render` writes `audio.wav` from the main play side alone, bit
//!   for bit what the project renders without the cue slot, and
//!   `devices/cue.wav` beside it; the render does not depend on the block
//!   size or the thread count.
//! - `signal play --driver null --route cue=3,4` plays both; without a route
//!   the cue is said to be not heard, and main plays.
//! - On the null driver with the Setup's sound system (main on 1–2, cue on
//!   3–4 of a scripted 4-channel SSL 2+), the Runner's frame is cue then
//!   main, and the channel map puts cue on device channels 3–4 and main on
//!   1–2.
//!
//! The committed project is only read; scratch copies go under the target's
//! temporary directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use signal_audio::{ChannelMap, ConfigRange, DeviceInfo, Direction};
use signal_core::Registry;
use signal_engine::{Engine, EngineConfig, RealtimeConfig};
use signal_output::{realtime_routed, render};
use signal_project::Project;
use signal_project::device::play_sides;
use signal_ui::player::{Backend, Output, open_routed};
use signal_ui::setup::{DeviceEntry, PlayEntry, Setup};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn project_dir() -> PathBuf {
    repository().join("projects/cue-mix")
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn signal(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_signal"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn the_render_writes_main_as_audio_wav_and_the_cue_beside_it() {
    let out = scratch("cue-mix-render");
    let run = signal(&[
        "--json",
        "render",
        project_dir().to_str().unwrap(),
        "--frames",
        "48000",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let report: Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(report["channels"], 2);
    assert_eq!(report["devices"]["cue"]["file"], "devices/cue.wav");
    assert_eq!(report["devices"]["cue"]["channels"], 2);
    let cue = hound::WavReader::open(out.join("devices/cue.wav")).unwrap();
    assert_eq!(cue.spec().channels, 2);
    assert_eq!(cue.len(), 48_000 * 2);

    // `audio.wav` is the main side alone: the project without its cue slot
    // renders the same samples.
    let text = fs::read_to_string(project_dir().join("project.toml")).unwrap();
    let without: String = text
        .lines()
        .filter(|l| !l.starts_with("cue =") && !l.contains("\"cue."))
        .map(|l| format!("{l}\n"))
        .collect();
    let alone = scratch("cue-mix-alone");
    fs::write(alone.join("project.toml"), without).unwrap();
    let registry = registry();
    let project = Project::load(&alone).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let mut engine = Engine::new(
        &plan,
        &registry,
        EngineConfig::new(48_000, project.seed),
        Vec::new(),
    )
    .unwrap();
    let rendered = render(&mut engine, 48_000).unwrap();
    assert_eq!(
        report["manifest"]["audio_fnv1a64"],
        format!("{:016x}", rendered.audio_fnv1a64())
    );

    // Any block size and thread count: the same files.
    let other = scratch("cue-mix-render-blocks");
    let run = signal(&[
        "--json",
        "render",
        project_dir().to_str().unwrap(),
        "--frames",
        "48000",
        "--block",
        "7",
        "--threads",
        "4",
        "--out",
        other.to_str().unwrap(),
    ]);
    assert!(run.status.success());
    let again: Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(
        again["manifest"]["audio_fnv1a64"],
        report["manifest"]["audio_fnv1a64"]
    );
    assert_eq!(again["devices"], report["devices"]);
}

#[test]
fn play_routes_the_cue_or_says_it_is_not_heard() {
    let dir = project_dir();
    let dir = dir.to_str().unwrap();
    let routed = signal(&[
        "play",
        dir,
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "0.5",
        "--route",
        "cue=3,4",
    ]);
    assert!(
        routed.status.success(),
        "{}",
        String::from_utf8_lossy(&routed.stderr)
    );
    assert!(!String::from_utf8_lossy(&routed.stderr).contains("not heard"));
    let unrouted = signal(&[
        "play",
        dir,
        "--driver",
        "null",
        "--fast",
        "--seconds",
        "0.5",
    ]);
    assert!(unrouted.status.success());
    assert!(
        String::from_utf8_lossy(&unrouted.stderr).contains("cue (/cue): not heard on this machine"),
        "{}",
        String::from_utf8_lossy(&unrouted.stderr)
    );
}

fn ssl() -> DeviceInfo {
    DeviceInfo {
        name: "SSL 2+ MKII".to_owned(),
        id: Some("coreaudio:ssl".to_owned()),
        direction: Direction::Output,
        default: true,
        rate: Some(48_000),
        configs: vec![ConfigRange {
            channels: 4,
            min_rate: 44_100,
            max_rate: 192_000,
            buffer: Some((15, 4096)),
            sample_format: "f32".to_owned(),
        }],
    }
}

/// The owner's case on a scripted SSL 2+: main on Out 1–2, cue on Out 3–4,
/// both heard; the frame (cue l, cue r, main l, main r) maps onto device
/// channels 3, 4, 1, 2.
#[test]
fn main_and_cue_play_on_out_1_2_and_out_3_4_of_one_device() {
    let registry = registry();
    let project = Project::load(&project_dir()).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    let sides = play_sides(&project, &plan);
    let devices = vec![ssl()];
    let mut setup = Setup {
        output: Some(DeviceEntry {
            name: "SSL 2+ MKII".to_owned(),
            id: Some("coreaudio:ssl".to_owned()),
            signature: String::new(),
        }),
        output_channels: Some(vec![1, 2]),
        ..Setup::default()
    };
    setup.play.insert(
        "cue".to_owned(),
        PlayEntry {
            channels: vec![3, 4],
            device: None,
        },
    );
    let mut notes = Vec::new();
    let (mut output, routing) = open_routed(
        &Backend::Null(devices.clone()),
        &devices,
        &setup,
        48_000,
        &sides,
        1,
        &mut notes,
    )
    .unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(routing.width(), 4);
    assert_eq!(
        (0..4).map(|c| output.target(c)).collect::<Vec<_>>(),
        [Some(2), Some(3), Some(0), Some(1)]
    );
    let (runner, _controller) = realtime_routed(
        &plan,
        &registry,
        RealtimeConfig::new(48_000, 1),
        None,
        &routing.addresses(),
    )
    .unwrap();
    output.start(runner).unwrap();
    let null = output.null().unwrap();
    null.run(24_000, || {}).unwrap();
    let frame = null.take_output();
    // What a device gets: the frame scattered by the map.
    let map = ChannelMap::with_routes(4, routing.targets()).unwrap();
    let mut device = vec![0.0f32; frame.len()];
    map.scatter(&frame, &mut device);
    let peak = |c: usize| {
        device
            .chunks_exact(4)
            .fold(0.0f32, |p, f| p.max(f[c].abs()))
    };
    assert!(peak(0) > 0.01 && peak(1) > 0.01, "main on Out 1–2");
    assert!(peak(2) > 0.01 && peak(3) > 0.01, "cue on Out 3–4");
    // Main on 1–2 is the main sink alone; cue (with its click) differs.
    assert_ne!(
        device.chunks_exact(4).map(|f| f[0]).collect::<Vec<_>>(),
        device.chunks_exact(4).map(|f| f[2]).collect::<Vec<_>>()
    );
    drop(Output::stop(&mut output));

    // Without [play.cue] the cue is not heard, and main keeps 1–2.
    setup.play.clear();
    let mut notes = Vec::new();
    let (output, _) = open_routed(
        &Backend::Null(devices.clone()),
        &devices,
        &setup,
        48_000,
        &sides,
        1,
        &mut notes,
    )
    .unwrap();
    assert_eq!(
        (0..4).map(|c| output.target(c)).collect::<Vec<_>>(),
        [None, None, Some(0), Some(1)]
    );
    assert_eq!(notes.len(), 1, "{notes:?}");
}

#[test]
fn ui_check_passes() {
    let run = signal(&["ui", "check", project_dir().to_str().unwrap()]);
    assert!(
        run.status.success(),
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
