//! The offline driver (AC7), `log.jsonl` (AC8), determinism, and the round trip
//! of `audio.wav` and `manifest.json` (AC9).

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use common::{addr, conn, engine, engine_seeded, ext, inst, inst_with, plan, ramp};
use signal_core::{EventPayload, Fnv1a64, Frame, fnv1a64};
use signal_engine::{Manifest, Provenance};
use signal_output::{Render, RenderError, render};

/// A fresh directory for one test's artifacts, inside the target directory.
fn artifacts(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn provenance() -> Provenance {
    Provenance {
        signal_revision: "cf613bd1fc85a9a8dc6de643e68080ea99ea1265".to_owned(),
        signal_dirty: Some(true),
        rustc_version: "rustc 1.99.0".to_owned(),
        project_file_hashes: [("project.toml".to_owned(), "0123456789abcdef".to_owned())]
            .into_iter()
            .collect(),
        library_composite_hashes: Default::default(),
        library_interface_hashes: Default::default(),
        included_projects: Vec::new(),
        cargo_lock_hash: "fedcba9876543210".to_owned(),
    }
}

/// Noise and an oscillator into the two channels: values f32 cannot hold exactly.
fn noisy_render(seed: u64) -> Render {
    let plan = plan(
        vec![
            inst("n", "test.noise"),
            inst_with("o", "test.osc", &[("freq", 440.0)]),
            inst("t", "test.trigger"),
            inst("c", "test.counter"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("n.out", "rec.zeta"),
            conn("o.out", "rec.alpha"),
            conn("t.out", "c.in"),
        ],
    );
    let mut engine = engine_seeded(
        &plan,
        48_000,
        seed,
        vec![ext(3, 1, "/c.in", EventPayload::Trigger)],
    );
    engine.suspend(&addr("/n"), Frame(10)).unwrap();
    engine.resume(&addr("/n"), Frame(20)).unwrap();
    render(&mut engine, 200).unwrap()
}

#[test]
fn render_records_single_audio_sink_in_input_port_order() {
    let plan = plan(
        vec![
            inst_with("k", "test.const", &[("value", 0.25)]),
            inst("d", "test.dc"),
            inst("rec", "test.recorder"),
        ],
        vec![conn("k.out", "rec.zeta"), conn("d.out", "rec.alpha")],
    );
    let rendered = render(&mut engine(&plan, 48_000, vec![]), 4).unwrap();
    assert_eq!(
        (rendered.channels, rendered.sample_rate, rendered.frames),
        (2, 48_000, 4)
    );
    // Channel 0 is `zeta`, declared first; channel 1 is `alpha`.
    assert_eq!(rendered.samples, [0.25, 0.75].repeat(4));
}

/// A sink with a `gain` parameter is recorded as gain · input, with the gain's
/// effective value in each frame (`std.audio_out`, docs/semantics.md §8).
#[test]
fn render_applies_the_sinks_gain_parameter() {
    let plan = plan(
        vec![
            inst_with("k", "test.const", &[("value", 0.25)]),
            inst("d", "test.dc"),
            inst_with("out", "test.gained_recorder", &[("gain", 0.5)]),
        ],
        vec![conn("k.out", "out.l"), conn("d.out", "out.r")],
    );
    let external = vec![ext(2, 1, "/out.gain", EventPayload::Set(2.0))];
    let rendered = render(&mut engine(&plan, 48_000, external), 4).unwrap();
    assert_eq!(rendered.channels, 2);
    assert_eq!(
        rendered.samples,
        [0.125, 0.375, 0.125, 0.375, 0.5, 1.5, 0.5, 1.5]
    );
}

#[test]
fn zero_or_several_audio_sinks_is_an_error() {
    let none = plan(vec![inst("d", "test.dc")], vec![]);
    assert!(matches!(
        render(&mut engine(&none, 48_000, vec![]), 1),
        Err(RenderError::NoAudioSink)
    ));

    let two = plan(
        vec![inst("r1", "test.recorder"), inst("r2", "test.recorder")],
        vec![],
    );
    match render(&mut engine(&two, 48_000, vec![]), 1) {
        Err(RenderError::SeveralAudioSinks { instances }) => {
            assert_eq!(instances, [addr("/r1"), addr("/r2")]);
        }
        other => panic!("expected SeveralAudioSinks, got {other:?}"),
    }

    let one = plan(vec![inst("rec", "test.recorder")], vec![]);
    let mut started = engine(&one, 48_000, vec![]);
    started.run_frame();
    assert!(matches!(
        render(&mut started, 1),
        Err(RenderError::AlreadyStarted)
    ));
}

/// One JSON object per line: every executed event with frame, target, payload
/// and source, plus suspensions, discards and faults, each frame in canonical
/// order (edits, then by target port, then faults).
#[test]
fn log_jsonl_records_events_faults_suspensions_and_discards() {
    let plan = plan(
        vec![
            inst("t", "test.trigger"),
            inst("c", "test.counter"),
            inst("p", "test.probe_add"),
            inst("f", "test.faulty"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("t.out", "c.in"),
            conn("f.out", "rec.zeta"),
            conn("c.out", "rec.alpha"),
        ],
    );
    let external = vec![
        ext(2, 1, "/p.value", EventPayload::Set(0.125)),
        ext(4, 2, "/p.value", ramp(0.5, 3)),
    ];
    let mut engine = engine(&plan, 48_000, external);
    engine.suspend(&addr("/c"), Frame(3)).unwrap();
    engine.resume(&addr("/c"), Frame(5)).unwrap();
    let rendered = render(&mut engine, 12).unwrap();

    let log = String::from_utf8(rendered.log).unwrap();
    assert!(log.ends_with('\n'));
    let lines: Vec<&str> = log.lines().collect();
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        let object = value.as_object().unwrap();
        if object["type"] == "event" || object["type"] == "discard" {
            for key in ["frame", "target", "payload", "source"] {
                assert!(object.contains_key(key), "{line}");
            }
        }
    }
    // 12 trigger deliveries (10 events, 2 discards), 2 externals, a suspend, a
    // resume and a fault.
    assert_eq!(lines.len(), 17);
    let expected_in_order = [
        r#"{"type":"event","frame":0,"target":"/c.in","payload":"trigger","source":{"port":"/t.out"}}"#,
        r#"{"type":"event","frame":2,"target":"/p.value","payload":{"set":0.125},"source":{"external":{"sequence":1}}}"#,
        r#"{"type":"suspend","frame":3,"address":"/c"}"#,
        r#"{"type":"discard","frame":3,"target":"/c.in","payload":"trigger","source":{"port":"/t.out"},"reason":"suspended"}"#,
        r#"{"type":"event","frame":4,"target":"/p.value","payload":{"ramp":{"to":0.5,"frames":3,"curve":"linear"}},"source":{"external":{"sequence":2}}}"#,
        r#"{"type":"resume","frame":5,"address":"/c","discarded":2}"#,
        r#"{"type":"fault","frame":8,"address":"/f","port":"/f.out","reason":"non_finite"}"#,
    ];
    let positions: Vec<usize> = expected_in_order
        .iter()
        .map(|expected| {
            lines
                .iter()
                .position(|l| l == expected)
                .unwrap_or_else(|| panic!("missing {expected}"))
        })
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    let types: BTreeSet<&str> = lines.iter().map(|l| l.split('"').nth(3).unwrap()).collect();
    assert_eq!(
        types,
        BTreeSet::from(["discard", "event", "fault", "resume", "suspend"])
    );
}

/// The same plan and seed give identical audio and log, and identical hashes;
/// another seed changes the audio.
#[test]
fn same_plan_and_seed_render_identical_hashes() {
    let (a, b) = (noisy_render(42), noisy_render(42));
    let bits = |r: &Render| r.samples.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&a), bits(&b));
    assert_eq!(a.log, b.log);
    assert_eq!(a.audio_fnv1a64(), b.audio_fnv1a64());
    assert_eq!(a.log_fnv1a64(), b.log_fnv1a64());
    assert_eq!(a.manifest(provenance()), b.manifest(provenance()));

    let other = noisy_render(43);
    assert_ne!(other.audio_fnv1a64(), a.audio_fnv1a64());
    assert_eq!(other.log_fnv1a64(), a.log_fnv1a64());
}

/// `audio.wav` reads back as IEEE float 32-bit at the sample rate with the
/// rendered samples (rounded to f32), and the manifest's hashes are those of
/// the files' contents.
#[test]
fn wav_round_trips_and_hashes_match_files() {
    let rendered = noisy_render(42);
    let dir = artifacts("signal-engine-wav-round-trip");
    let manifest = rendered.write(&dir, provenance()).unwrap();

    let mut reader = hound::WavReader::open(dir.join("audio.wav")).unwrap();
    assert_eq!(
        reader.spec(),
        hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        }
    );
    let read: Vec<f32> = reader.samples::<f32>().collect::<Result<_, _>>().unwrap();
    let written: Vec<u32> = rendered
        .samples
        .iter()
        .map(|&x| (x as f32).to_bits())
        .collect();
    assert_eq!(
        read.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        written
    );

    let mut audio = Fnv1a64::new();
    for sample in &read {
        audio.update(&sample.to_le_bytes());
    }
    assert_eq!(manifest.audio_fnv1a64, format!("{:016x}", audio.finish()));
    let log = std::fs::read(dir.join("log.jsonl")).unwrap();
    assert_eq!(log, rendered.log);
    assert_eq!(manifest.log_fnv1a64, format!("{:016x}", fnv1a64(&log)));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `manifest.json` has every caller-supplied and engine-filled field and
/// reads back as the same manifest.
#[test]
fn manifest_serializes_every_field() {
    let rendered = noisy_render(42);
    let dir = artifacts("signal-engine-manifest");
    let manifest = rendered.write(&dir, provenance()).unwrap();
    let text = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
    assert!(text.ends_with('\n'));

    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let keys = |v: &serde_json::Value| {
        v.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    let expected = [
        "provenance",
        "seed",
        "sample_rate",
        "frames",
        "debug_assertions",
        "log_fnv1a64",
        "audio_fnv1a64",
    ];
    assert_eq!(
        keys(&value),
        expected.iter().map(|k| k.to_string()).collect()
    );
    let supplied = [
        "signal_revision",
        "signal_dirty",
        "rustc_version",
        "project_file_hashes",
        "cargo_lock_hash",
    ];
    assert_eq!(
        keys(&value["provenance"]),
        supplied.iter().map(|k| k.to_string()).collect()
    );
    assert_eq!(value["seed"], 42);
    assert_eq!(value["sample_rate"], 48_000);
    assert_eq!(value["frames"], 200);
    assert_eq!(value["debug_assertions"], cfg!(debug_assertions));
    for hash in ["log_fnv1a64", "audio_fnv1a64"] {
        let text = value[hash].as_str().unwrap();
        assert_eq!(text.len(), 16);
        assert!(
            text.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    let read: Manifest = serde_json::from_str(&text).unwrap();
    assert_eq!(read, manifest);
    assert_eq!(read.provenance, provenance());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `signal_dirty` is written `true`, `false` or `"unknown"` and reads back the
/// same; any other text is refused.
#[test]
fn signal_dirty_is_true_false_or_unknown() {
    for (dirty, json) in [
        (Some(true), serde_json::json!(true)),
        (Some(false), serde_json::json!(false)),
        (None, serde_json::json!("unknown")),
    ] {
        let provenance = Provenance {
            signal_dirty: dirty,
            ..provenance()
        };
        let value = serde_json::to_value(&provenance).unwrap();
        assert_eq!(value["signal_dirty"], json);
        let read: Provenance = serde_json::from_value(value).unwrap();
        assert_eq!(read, provenance);
    }
    let mut value = serde_json::to_value(provenance()).unwrap();
    value["signal_dirty"] = serde_json::json!("maybe");
    assert!(serde_json::from_value::<Provenance>(value).is_err());
}
