//! ADR 0038 §7: sources. A driver writes a source's external rows before
//! each block (`SinkOutput::sources`); `std.audio_in` puts them on its
//! outputs. The engine's output equals the oracle's, given the same frames
//! in its script, whatever the block size or thread count, and a source the
//! driver writes nothing for reads 0.

mod common;

use std::fs;
use std::path::PathBuf;

use common::{graph_of, registry};
use signal_core::{InstanceAddress, SinkKind};
use signal_engine::{Engine, EngineConfig, InstanceId, Schedule, SinkOutput, run_offline};
use signal_oracle::{Script, SourceInput};
use signal_project::Project;

const PROJECT: &str = r#"schema_version = 1
[project]
name = "listen"
seed = 4
[instances]
mic = { module = "std.audio_in" }
mono = { module = "std.audio_in_mono" }
hiss = { module = "std.noise" }
sum = { module = "std.add" }
out = { module = "std.audio_out", gain = 0.5 }
[wiring]
connections = [
  { from = "mic.l", to = "sum.a" },
  { from = "hiss.out", to = "sum.b", scale = 0.01 },
  { from = "sum.out", to = "out.l" },
  { from = "mic.r", to = "out.r", scale = -0.25 },
]
"#;

const FRAMES: u64 = 3_000;

/// The driver's frames: row `r` of the source at frame `n`.
fn captured(r: usize, n: u64) -> f64 {
    let t = n as f64 / 48_000.0;
    (std::f64::consts::TAU * (220.0 + 110.0 * r as f64) * t).sin() * 0.8
}

/// A driver that writes `mic`'s rows and collects the audio sink's inputs
/// times its `gain`, as the offline driver records them.
struct Driver {
    mic: Option<InstanceId>,
    sink: Option<InstanceId>,
    gain: Option<usize>,
    samples: Vec<f64>,
}

impl SinkOutput for Driver {
    fn bind(&mut self, engine: &Engine) -> bool {
        let mic: InstanceAddress = "/mic".parse().unwrap();
        self.mic = engine.sources().find(|s| *s.address == mic).map(|s| s.id);
        self.sink = engine
            .sinks()
            .find(|s| s.kind == SinkKind::Audio)
            .map(|s| s.id);
        self.gain = engine.parameter_index(&"/out.gain".parse().unwrap());
        self.mic.is_some() && self.sink.is_some()
    }

    fn sources(&mut self, engine: &mut Engine, frames: usize) {
        let start = engine.next_frame().0;
        let k = engine.capacity();
        let Some(rows) = self.mic.and_then(|id| engine.source_rows_mut(id)) else {
            return;
        };
        for r in 0..2 {
            for f in 0..frames {
                rows[r * k + f] = captured(r, start + f as u64);
            }
        }
    }

    fn block(&mut self, engine: &Engine, frames: usize) {
        let (Some(sink), Some(gain)) = (self.sink, self.gain) else {
            return;
        };
        for f in 0..frames {
            let g = engine.parameter_at_frame(gain, f).unwrap();
            for c in 0..2 {
                self.samples
                    .push(g * engine.sink_input_at(sink, c, f).unwrap());
            }
        }
    }
}

#[test]
fn the_engine_reads_sources_as_the_oracle_does_in_any_blocks_and_threads() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("oracle-sources");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("project.toml"), PROJECT).unwrap();
    let registry = registry();
    let plan = Project::load(&dir).unwrap().plan(&registry).unwrap().plan;
    let script = Script {
        sample_rate: 48_000,
        seed: 4,
        frames: FRAMES,
        events: Vec::new(),
        edits: Vec::new(),
        swaps: Vec::new(),
        inputs: vec![SourceInput {
            instance: "/mic".parse().unwrap(),
            rows: (0..2)
                .map(|r| (0..FRAMES).map(|n| captured(r, n)).collect())
                .collect(),
        }],
    };
    let oracle = signal_oracle::run(&registry, &graph_of(&plan, 4), &script).unwrap();
    // The source reaches the output: not silence.
    assert!(oracle.samples.iter().any(|x| x.abs() > 0.1));
    for (block, threads) in [(1, 1), (7, 1), (64, 2), (256, 4)] {
        let config = EngineConfig {
            threads,
            schedule: if threads > 1 {
                Schedule::FORCED
            } else {
                Schedule::DEFAULT
            },
            ..EngineConfig::new(48_000, 4)
        };
        let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
        let mut driver = Driver {
            mic: None,
            sink: None,
            gain: None,
            samples: Vec::new(),
        };
        run_offline(&mut engine, FRAMES, block, |_| Ok(None), &mut driver).unwrap();
        assert_eq!(driver.samples.len(), oracle.samples.len());
        for (n, (a, b)) in driver.samples.iter().zip(&oracle.samples).enumerate() {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "block {block}, threads {threads}, sample {n}"
            );
        }
    }
}

/// A driver that writes nothing: every source reads 0, so the left channel
/// is the noise alone and the right one silence.
#[test]
fn a_source_the_driver_writes_nothing_for_reads_0() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("oracle-sources-silent");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("project.toml"), PROJECT).unwrap();
    let registry = registry();
    let plan = Project::load(&dir).unwrap().plan(&registry).unwrap().plan;
    let mut engine =
        Engine::new(&plan, &registry, EngineConfig::new(48_000, 4), Vec::new()).unwrap();
    struct Silent(Driver);
    impl SinkOutput for Silent {
        fn bind(&mut self, engine: &Engine) -> bool {
            self.0.bind(engine)
        }
        fn block(&mut self, engine: &Engine, frames: usize) {
            self.0.block(engine, frames);
        }
    }
    let mut silent = Silent(Driver {
        mic: None,
        sink: None,
        gain: None,
        samples: Vec::new(),
    });
    run_offline(&mut engine, 512, 64, |_| Ok(None), &mut silent).unwrap();
    let samples = silent.0.samples;
    assert!(samples.chunks(2).all(|f| f[1] == 0.0));
    assert!(samples.chunks(2).any(|f| f[0] != 0.0));
    assert!(samples.chunks(2).all(|f| f[0].abs() < 0.01));
}
