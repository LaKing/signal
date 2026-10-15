//! M2.7-9 (ADR 0022): moving the audio output policy out of the engine
//! changes no output. Each case's hashes were computed on the base revision
//! (581dbe2, before the move) with this same code, its imports and call
//! shapes aside, and are pinned here: offline renders (plain f32), a render
//! whose sink `gain` ramps into clipping, the null driver with 2 and 3 device
//! channels (samples, clipped count, meters), and the exact replay of a
//! recorded run that clipped: as the device played it when its session
//! records the device conversion, and unchanged for a session without it.

use std::fs;
use std::path::{Path, PathBuf};

use signal_core::{Curve, EventPayload, Fnv1a64, Frame, PortAddress, Registry};
use signal_engine::{Engine, EngineConfig, ExternalEvent, Provenance, RealtimeConfig};
use signal_output::{Conversion, Driver, Meter, NullDriver, realtime, render};
use signal_project::session::{LiveRecorder, Session, SessionRecorder, replay};
use signal_project::{Project, SourceFile};

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn f32_hash(samples: &[f32]) -> String {
    let mut hasher = Fnv1a64::new();
    for sample in samples {
        hasher.update(&sample.to_le_bytes());
    }
    format!("{:016x}", hasher.finish())
}

/// A sine into both inputs of the sink at `gain`.
fn loud(gain: f64) -> Project {
    let text = format!(
        "schema_version = 1\n\n[project]\nname = \"loud\"\nsample_rate = 48000\nseed = 7\n\n\
         [instances]\ntone = {{ module = \"std.sine_osc\", freq = 330 }}\n\
         out = {{ module = \"std.audio_out\", gain = {gain} }}\n\n\
         [wiring]\nconnections = [\n  {{ from = \"tone.out\", to = \"out.l\" }},\n  \
         {{ from = \"tone.out\", to = \"out.r\" }},\n]\n"
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap()
}

/// (audio, log, audio as the device converts it) hashes of an offline render
/// of `frames` frames.
fn rendered(
    project: &Project,
    frames: u64,
    external: Vec<ExternalEvent>,
) -> (String, String, String) {
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let config = EngineConfig::new(project.sample_rate, project.seed);
    let mut engine = Engine::new(&plan, &registry, config, external).unwrap();
    let mut render = render(&mut engine, frames).unwrap();
    let plain = format!("{:016x}", render.audio_fnv1a64());
    render.conversion = Conversion::Device;
    (
        plain,
        format!("{:016x}", render.log_fnv1a64()),
        format!("{:016x}", render.audio_fnv1a64()),
    )
}

#[test]
fn offline_renders_are_unchanged() {
    let kick = Project::load(&repository().join("projects/kick")).unwrap();
    let pulse = Project::load(&repository().join("projects/pulse")).unwrap();
    assert_eq!(
        rendered(&kick, 96_000, Vec::new()),
        (
            "01b87fbc1854a9b9".to_owned(),
            "ea16ce98bb2f93a4".to_owned(),
            "01b87fbc1854a9b9".to_owned()
        )
    );
    assert_eq!(
        rendered(&pulse, 48_000, Vec::new()),
        (
            "75f63398294141f5".to_owned(),
            "2e0c558ec76ec4e1".to_owned(),
            "75f63398294141f5".to_owned()
        )
    );
}

/// The sink's `gain` applied frame by frame while it ramps from 1 to 3.
#[test]
fn a_gain_ramp_renders_unchanged() {
    let ramp = ExternalEvent {
        frame: Frame(1_000),
        sequence: 0,
        target: "/out.gain".parse::<PortAddress>().unwrap(),
        payload: EventPayload::Ramp {
            to: 3.0,
            frames: 4_800,
            curve: Curve::Linear,
        },
    };
    assert_eq!(
        rendered(&loud(1.0), 9_600, vec![ramp]),
        (
            "8846ed118d2fb741".to_owned(),
            "18f126be0527c1d9".to_owned(),
            "14b7723f6e798e65".to_owned()
        )
    );
}

/// Meters as the control thread receives them.
#[derive(Default)]
struct Meters(Fnv1a64, u64);

impl Meters {
    fn meter(&mut self, meter: &Meter) {
        self.0.update(&meter.first_frame.0.to_le_bytes());
        self.0.update(&meter.frames.to_le_bytes());
        self.0.update(&(meter.channel as u64).to_le_bytes());
        self.0.update(&meter.peak.to_bits().to_le_bytes());
        self.0.update(&meter.sum_squares.to_bits().to_le_bytes());
        self.1 += 1;
    }
}

/// The null driver: (samples hash, clipped samples, meters hash).
fn played(project: &Project, channels: usize, frames: u64) -> (String, u64, String) {
    let registry = registry();
    let plan = project.plan(&registry).unwrap().plan;
    let mut config = RealtimeConfig::new(project.sample_rate, project.seed);
    config.meters = 1 << 16;
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let mut driver = NullDriver::new(project.sample_rate, channels, 11);
    driver.start(runner).unwrap();
    let mut meters = Meters::default();
    driver
        .run(frames, || {
            controller.poll_with(&mut (), |m| meters.meter(m))
        })
        .unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll_with(&mut (), |m| meters.meter(m));
    assert!(meters.1 > 0);
    let clipped = runner.clipped();
    (
        f32_hash(driver.output()),
        clipped,
        format!("{:016x}", meters.0.finish()),
    )
}

#[test]
fn the_null_driver_plays_unchanged() {
    let kick = Project::load(&repository().join("projects/kick")).unwrap();
    assert_eq!(
        played(&kick, 3, 48_000),
        (
            "5ccd5edb29d7e915".to_owned(),
            0,
            "27e50b0c9f04dcec".to_owned()
        )
    );
    assert_eq!(
        played(&loud(2.5), 2, 24_000),
        (
            "01ed41583c700485".to_owned(),
            35460,
            "3445ba4516f4f22d".to_owned()
        )
    );
}

/// A recorded run that clipped, and its replay (without latent modules).
/// The session records the device conversion, and its replay is what the
/// device played, clipped count included. With the `conversion` key removed,
/// as in a session recorded before it, the replay is a plain render whose
/// hash is the one pinned on the base revision: unchanged.
#[test]
fn an_exact_replay_is_unchanged() {
    let registry = registry();
    let project = loud(2.5);
    let mut plan = project.plan(&registry).unwrap().plan;
    plan.version = 0;
    let config = RealtimeConfig::new(project.sample_rate, project.seed);
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let recorder = SessionRecorder::new(
        &project,
        &registry,
        project.sample_rate,
        project.seed,
        Provenance::default(),
    )
    .unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let mut driver = NullDriver::new(project.sample_rate, 2, 5);
    driver.start(runner).unwrap();
    driver.run(12_000, || controller.poll(&mut live)).unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.finish(runner.next_frame()).unwrap();
    let recorder = live.into_recorder();
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("output_identity");
    let dir = root.join("replay");
    let legacy = root.join("replay-before-conversion");
    for dir in [&dir, &legacy] {
        let _ = fs::remove_dir_all(dir);
        recorder.write(dir).unwrap();
    }
    assert_eq!(f32_hash(driver.output()), "ad66631bac81097d");
    assert!(runner.clipped() > 0);

    let session = Session::load(&dir, &registry).unwrap();
    assert_eq!(session.conversion, Some(Conversion::Device));
    let replayed = replay(&session, &registry, None, 1).unwrap();
    assert_eq!(replayed.render.conversion, Conversion::Device);
    assert_eq!(
        format!("{:016x}", replayed.render.audio_fnv1a64()),
        "ad66631bac81097d"
    );
    assert_eq!(replayed.render.clipped(), runner.clipped());
    let manifest = replayed.render.manifest(Provenance::default());
    assert!(manifest.device_conversion);

    let path = legacy.join("session.toml");
    let text = fs::read_to_string(&path).unwrap();
    let line = "conversion = \"device\"\n";
    assert!(text.contains(line), "{text}");
    fs::write(&path, text.replace(line, "")).unwrap();
    let session = Session::load(&legacy, &registry).unwrap();
    assert_eq!(session.conversion, None);
    let replayed = replay(&session, &registry, None, 1).unwrap();
    assert_eq!(replayed.render.conversion, Conversion::Plain);
    assert_eq!(
        format!("{:016x}", replayed.render.audio_fnv1a64()),
        "318d8f3e3265e295"
    );
    assert_eq!(replayed.render.clipped(), 0);
    let manifest = replayed.render.manifest(Provenance::default());
    assert!(!manifest.device_conversion);
}
