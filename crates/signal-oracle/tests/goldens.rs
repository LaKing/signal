//! AC3 (M2.7-6): the oracle over the goldens' projects. Its audio, as
//! `audio.wav` stores it, has the golden's hash, and its audio, log and end
//! state equal the engine's render bit for bit. Debug builds compare the
//! first `DEBUG_FRAMES` frames with the engine (the oracle is slow
//! unoptimised); release builds (CI's `release-tests` job) the whole render,
//! against the golden too.

mod common;

use std::path::PathBuf;

use common::{Setup, compare, graph_of, registry, run_engine};
use serde_json::Value;
use signal_core::Fnv1a64;
use signal_oracle::Script;
use signal_project::Project;

/// The frames a debug build compares.
const DEBUG_FRAMES: u64 = 192_000;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden(name: &str) {
    let root = repository();
    let text =
        std::fs::read_to_string(root.join(format!("tests/golden/{name}/manifest.json"))).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    let registry = registry();
    let project = Project::load(&root.join(format!("projects/{name}"))).unwrap();
    let planned = project.plan(&registry).expect("the project is valid");
    let all = manifest["frames"].as_u64().unwrap();
    let frames = if cfg!(debug_assertions) {
        all.min(DEBUG_FRAMES)
    } else {
        all
    };
    let script = Script {
        sample_rate: u32::try_from(manifest["sample_rate"].as_u64().unwrap()).unwrap(),
        seed: manifest["seed"].as_u64().unwrap(),
        frames,
        events: Vec::new(),
        edits: Vec::new(),
        swaps: Vec::new(),
    };
    let oracle = signal_oracle::run(&registry, &graph_of(&planned.plan, 7), &script).unwrap();
    let setup = Setup {
        block: 256,
        threads: 1,
        splits: Vec::new(),
    };
    let engine = run_engine(&registry, &planned.plan, &[], &script, &setup, &oracle);
    if let Err(difference) = compare(&oracle, &engine) {
        panic!("{name}: the engine differs from the oracle: {difference}");
    }
    if frames == all {
        let mut hash = Fnv1a64::new();
        for &sample in &oracle.samples {
            hash.update(&(sample as f32).to_le_bytes());
        }
        assert_eq!(
            format!("{:016x}", hash.finish()),
            manifest["audio_fnv1a64"].as_str().unwrap(),
            "{name}: the oracle's audio is not the golden's"
        );
    }
    println!(
        "{name}: oracle = engine over {frames} of {all} frames ({} log lines){}",
        oracle.log.len(),
        if frames == all {
            ", audio hash = golden"
        } else {
            ""
        }
    );
}

#[test]
fn the_oracle_renders_the_kick_golden() {
    golden("kick");
}

#[test]
fn the_oracle_renders_the_pulse_golden() {
    golden("pulse");
}
