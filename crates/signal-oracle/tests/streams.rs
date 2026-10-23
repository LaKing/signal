//! ADR 0035 §4: the oracle derives an instance's random stream from its
//! stream key, as the engine does, so oracle parity holds on a plan with
//! scope roots (a project including another, which draws noise).

mod common;

use std::fs;
use std::path::PathBuf;

use common::{Setup, compare, graph_of, registry, run_engine};
use signal_oracle::Script;
use signal_project::Project;

const NOISY: &str = r#"schema_version = 1
[project]
name = "noisy"
seed = 21
[instances]
a = { module = "std.noise" }
z = { module = "std.noise" }
out = { module = "std.audio_out", gain = 0.5 }
[wiring]
connections = [
  { from = "a.out", to = "out.l", scale = 0.3 },
  { from = "z.out", to = "out.l", scale = 0.2 },
  { from = "z.out", to = "out.r" },
]
"#;

const HOST: &str = r#"schema_version = 1
[project]
name = "host"
seed = 99
[instances]
m = { project = "noisy" }
own = { module = "std.noise" }
out = { module = "std.audio_out" }
[wiring]
connections = [
  { from = "m.l", to = "out.l" },
  { from = "m.r", to = "out.r" },
  { from = "own.out", to = "out.r", scale = 0.1 },
]
"#;

#[test]
fn the_oracle_derives_streams_from_their_keys() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("oracle-streams");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("noisy")).unwrap();
    fs::write(dir.join("project.toml"), HOST).unwrap();
    fs::write(dir.join("noisy/project.toml"), NOISY).unwrap();
    let registry = registry();
    let project = Project::load(&dir).unwrap();
    let plan = project.plan(&registry).unwrap().plan;
    assert!(plan.instances.iter().any(|i| i.stream.is_some()));
    let script = Script {
        sample_rate: 48_000,
        seed: 99,
        frames: 4_800,
        events: Vec::new(),
        edits: Vec::new(),
        swaps: Vec::new(),
    };
    let graph = graph_of(&plan, 3);
    let oracle = signal_oracle::run(&registry, &graph, &script).unwrap();
    for (block, threads) in [(1, 1), (64, 2), (256, 4)] {
        let setup = Setup {
            block,
            threads,
            splits: Vec::new(),
        };
        let engine = run_engine(&registry, &plan, &[], &script, &setup, &oracle);
        if let Err(difference) = compare(&oracle, &engine) {
            panic!("block {block}, threads {threads}: {difference}");
        }
    }
    // Without the keys the oracle draws other streams: the keys are read.
    let mut keyless = graph.clone();
    for instance in &mut keyless.instances {
        instance.stream = None;
    }
    let other = signal_oracle::run(&registry, &keyless, &script).unwrap();
    assert_ne!(other.samples, oracle.samples);
}
