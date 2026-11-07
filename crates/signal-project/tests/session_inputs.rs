//! ADR 0038 §9: live input in sessions. A null-driver run whose capture
//! side (`/mic`, `std.audio_in`) reads a [`RecordedInput`] records what the
//! source read into `inputs/mic.f32`, drained as the run goes, and the
//! session replays to what was played, bit for bit. A session without the
//! record, or with an incomplete one, refuses an exact replay (an ideal
//! render, or `allow_incomplete`, reads 0 where it is missing); a run that
//! captured nothing records `zeros`; a changed file fails to load.

use std::fs;
use std::path::PathBuf;

use signal_core::Registry;
use signal_engine::{ControlSink, RealtimeConfig};
use signal_output::inputs::{RecordedInput, RunInput};
use signal_output::{Driver, NullDriver, realtime};
use signal_project::session::{
    InputDevice, LiveRecorder, ReplayError, ReplayOptions, Session, SessionRecorder, replay,
    replay_with,
};
use signal_project::{Project, SourceFile, provenance};

const RATE: u32 = 48_000;
const FRAMES: u64 = 8_192;

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
}

/// The mic's left channel plus a tone on the left, its right channel on
/// the right, through `std.audio_out` at gain 0.5.
fn project() -> Project {
    let text = format!(
        "schema_version = 1\n[project]\nname = \"inputs\"\nsample_rate = {RATE}\nseed = 2\n\
         [instances]\n\
         mic = {{ device = {{ capture = \"stereo\", role = \"main\" }} }}\n\
         tone = {{ module = \"std.sine_osc\", freq = 330 }}\n\
         sum = {{ module = \"std.add\" }}\n\
         out = {{ module = \"std.audio_out\", gain = 0.5 }}\n\
         [wiring]\nconnections = [\n\
           {{ from = \"mic.l\", to = \"sum.a\" }},\n\
           {{ from = \"tone.out\", to = \"sum.b\", scale = 0.25 }},\n\
           {{ from = \"sum.out\", to = \"out.l\" }},\n\
           {{ from = \"mic.r\", to = \"out.r\" }},\n]\n"
    );
    Project::from_sources(vec![SourceFile {
        path: "project.toml".to_owned(),
        text,
    }])
    .unwrap()
}

/// The words the mic delivers: a different sine per channel.
fn captured() -> Vec<f32> {
    (0..FRAMES as usize * 2)
        .map(|i| {
            let (n, c) = ((i / 2) as f32, (i % 2) as f32);
            (n * (0.01 + 0.007 * c)).sin() * 0.6
        })
        .collect()
}

/// How the run records its input.
enum Capture {
    /// A record ring of `ring` frames, drained at every poll (`with_inputs`).
    Recorded { ring: usize },
    /// The mic plays but the recorder is not given its record.
    NotGiven,
    /// Nothing captured (as the null driver): `with_inputs` with no record.
    Nothing,
}

/// A null-driver run of [`project`] for [`FRAMES`] frames in callbacks of
/// 256, polled between them; its session written to `name`; what it played.
fn record(name: &str, capture: Capture) -> (PathBuf, Vec<f32>) {
    let registry = registry();
    let project = project();
    let mut plan = project.plan(&registry).unwrap().plan;
    plan.version = 0;
    let (mut runner, mut controller) =
        realtime(&plan, &registry, RealtimeConfig::new(RATE, project.seed)).unwrap();
    let provenance = provenance::collect(&provenance::source_dir(), &project.files);
    let recorder =
        SessionRecorder::new(&project, &registry, RATE, project.seed, provenance).unwrap();
    let mut live = LiveRecorder::new(recorder, registry.clone());
    let ring = match capture {
        Capture::Recorded { ring } => Some(ring),
        _ => None,
    };
    if !matches!(capture, Capture::Nothing) {
        let (input, record) = RunInput::new(
            "/mic".parse().unwrap(),
            2,
            Box::new(RecordedInput::new(captured(), 2)),
            1_024,
            ring.or(Some(1)),
        );
        runner.set_inputs(vec![input]);
        if ring.is_some() {
            let device = InputDevice {
                name: "Test mic".to_owned(),
                channels: vec![1, 2],
            };
            live.set_inputs(vec![(record.unwrap(), Some(device))]);
        }
    } else {
        live.set_inputs(Vec::new());
    }
    let mut driver = NullDriver::new(RATE, 2, 3);
    driver.start(runner).unwrap();
    for _ in 0..FRAMES / 256 {
        driver.run(256, || {}).unwrap();
        controller.poll(&mut live);
    }
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut live);
    live.status(&runner.status());
    live.finish(runner.next_frame()).unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("session-inputs/{name}"));
    let _ = fs::remove_dir_all(&dir);
    live.into_recorder().write(&dir).unwrap();
    (dir, driver.output().to_vec())
}

#[test]
fn a_recorded_input_replays_to_what_was_played() {
    let registry = registry();
    // A ring of 1 024 frames, far less than the run: drained as it goes.
    let (dir, played) = record("recorded", Capture::Recorded { ring: 1_024 });
    let session = Session::load(&dir, &registry).unwrap();
    assert_eq!(session.inputs.len(), 1);
    let input = &session.inputs[0];
    assert!(input.complete, "{input:?}");
    assert_eq!(input.frames, FRAMES);
    assert_eq!(input.words.as_deref(), Some(&captured()[..]));
    assert_eq!(input.device.as_ref().unwrap().name, "Test mic");
    assert!(dir.join("inputs/mic.f32").is_file());
    let text = fs::read_to_string(dir.join("session.toml")).unwrap();
    assert!(text.contains("[[inputs]]"), "{text}");

    let replayed = replay(&session, &registry, None, 1).unwrap();
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    // The mic is heard: not the tone alone.
    assert!(played.chunks(2).any(|f| f[1].abs() > 0.1));
    assert_eq!(samples, played, "replay equals what was played");
    assert!(replayed.incomplete_inputs.is_empty());
    // On four threads too.
    let replayed = replay(&session, &registry, None, 4).unwrap();
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(samples, played);
    // An exact replay renders nothing past the recorded end.
    let error = replay(&session, &registry, Some(FRAMES + 1), 1)
        .err()
        .unwrap();
    assert!(
        matches!(error, ReplayError::BeyondRecordedEnd { .. }),
        "{error}"
    );
}

#[test]
fn a_session_without_its_input_record_refuses_an_exact_replay() {
    let registry = registry();
    let (dir, _) = record("not-given", Capture::NotGiven);
    let session = Session::load(&dir, &registry).unwrap();
    assert!(session.inputs.is_empty());
    let error = replay(&session, &registry, None, 1).err().unwrap();
    assert!(
        matches!(&error, ReplayError::InputNotRecorded { address } if address.to_string() == "/mic"),
        "{error}"
    );
    assert!(error.to_string().contains("--ideal"));
    // Ideally: the input as silence, and said so.
    let options = ReplayOptions {
        ideal: true,
        threads: 1,
        ..ReplayOptions::default()
    };
    let replayed = replay_with(&session, &registry, options).unwrap();
    assert_eq!(replayed.incomplete_inputs.len(), 1);
    assert!(replayed.render.samples.chunks(2).all(|f| f[1] == 0.0));
}

#[test]
fn an_incomplete_input_record_refuses_unless_allowed() {
    let registry = registry();
    // A ring of 64 frames against callbacks of 256: it overflows.
    let (dir, played) = record("incomplete", Capture::Recorded { ring: 64 });
    let session = Session::load(&dir, &registry).unwrap();
    let input = &session.inputs[0];
    assert!(!input.complete);
    assert!(
        input.reason.as_ref().unwrap().contains("fell behind"),
        "{input:?}"
    );
    assert!(input.frames < FRAMES);
    let error = replay(&session, &registry, None, 1).err().unwrap();
    assert!(
        matches!(error, ReplayError::IncompleteInput { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("--allow-incomplete"));
    let options = ReplayOptions {
        allow_incomplete: true,
        threads: 1,
        ..ReplayOptions::default()
    };
    let replayed = replay_with(&session, &registry, options).unwrap();
    assert_eq!(replayed.incomplete_inputs.len(), 1);
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_ne!(samples, played, "what was lost is not replayed");
}

#[test]
fn a_run_that_captured_nothing_records_zeros() {
    let registry = registry();
    let (dir, played) = record("zeros", Capture::Nothing);
    let session = Session::load(&dir, &registry).unwrap();
    assert_eq!(session.inputs.len(), 1);
    assert_eq!(session.inputs[0].words, None);
    assert!(session.inputs[0].complete);
    assert!(!dir.join("inputs").exists());
    let text = fs::read_to_string(dir.join("session.toml")).unwrap();
    assert!(text.contains("zeros = true"), "{text}");
    let replayed = replay(&session, &registry, None, 1).unwrap();
    let samples: Vec<f32> = replayed.render.samples.iter().map(|&x| x as f32).collect();
    assert_eq!(samples, played);
    assert!(played.chunks(2).all(|f| f[1] == 0.0));
}

#[test]
fn a_changed_input_file_fails_to_load() {
    let registry = registry();
    let (dir, _) = record("changed", Capture::Recorded { ring: 1_024 });
    let path = dir.join("inputs/mic.f32");
    let mut bytes = fs::read(&path).unwrap();
    bytes[100] ^= 1;
    fs::write(&path, &bytes).unwrap();
    let error = Session::load(&dir, &registry).err().unwrap();
    assert!(format!("{error:?}").contains("FNV-1a 64"), "{error:?}");
    // Shorter than its frames.
    bytes.truncate(bytes.len() - 8);
    fs::write(&path, &bytes).unwrap();
    let error = Session::load(&dir, &registry).err().unwrap();
    assert!(format!("{error:?}").contains("bytes, not"), "{error:?}");
}
