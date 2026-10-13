//! ADR 0019 §3 (M2.6-4): modules with a declared latency, without a GPU.
//! `test.latent` declares 1 block of 4 frames and `same_machine`, and its
//! due result "arrives" only on the third `due` of a block, like a slow
//! device. Offline the engine waits for it (exact, for every *k* and thread
//! count); in real time it asks once, gives up, and logs and counts a
//! `late_result` fault (silence, not a mute). The registry refuses a bad
//! declaration, and the manifest records the `same_machine` instance.

mod common;

use common::{MIXED_BLOCK, conn, inst, plan, port_addr};
use signal_core::{
    BlockLatency, Direction, Due, Feedthrough, Frame, Inputs, InstanceAddress, LossCause,
    ModuleDescriptor, ModuleType, Outputs, PortDescriptor, PortKind, PortShape, PrepareContext,
    Primitive, RateDivisor, Registry, RegistryError, Reproducibility, StateOutputPrimitive,
};
use signal_engine::{
    BoundaryLine, BoundaryOutcome, ControlSink, Driver, Engine, EngineConfig, FaultReason, LogLine,
    NullDriver, RealtimeConfig, SameMachine, Schedule, Scripted, realtime, render,
};
use signal_graph::{CompositeDef, Exports, build_plan};

const fn port(name: &'static str, direction: Direction) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape: PortShape::Signal,
        rate: RateDivisor::ONE,
        kind: PortKind::Audio,
        unit: None,
        range: None,
    }
}

const BLOCK: usize = 4;

const LATENT: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.latent",
    doc: "out in block m = Σ in over block m − 1; slow to arrive",
    ports: &[port("in", Direction::In), port("out", Direction::Out)],
    parameters: &[],
    feedthrough: Feedthrough::StateOutput,
    sink: None,
    latency: Some(BlockLatency {
        latency_blocks: 1,
        block_frames: BLOCK as u32,
    }),
    reproducibility: Reproducibility::SameMachine,
};

/// `due` polls before a block's result arrives.
const POLLS: u32 = 3;

#[derive(Default)]
struct Latent {
    frame: usize,
    block: u64,
    sum: f64,
    pending: f64,
    shown: f64,
    polls: u32,
    resolved: bool,
}

impl StateOutputPrimitive for Latent {
    fn describe() -> ModuleDescriptor {
        LATENT
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        *self = Latent::default();
    }
    fn reset(&mut self) {
        *self = Latent::default();
    }
    fn due(&mut self) -> Due {
        if self.frame != 0 || self.resolved {
            return Due::Ready;
        }
        if self.block == 0 {
            self.resolved = true;
            return Due::Ready;
        }
        self.polls += 1;
        if self.polls < POLLS {
            return Due::Pending;
        }
        self.shown = self.pending;
        self.resolved = true;
        Due::Ready
    }
    fn miss(&mut self) {
        self.shown = 0.0;
        self.resolved = true;
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(self.shown);
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        self.sum += inputs.signal(0).first().copied().unwrap_or(0.0);
        self.frame += 1;
        if self.frame == BLOCK {
            self.pending = self.sum;
            self.sum = 0.0;
            self.frame = 0;
            self.block += 1;
            self.resolved = false;
            self.polls = 0;
        }
    }
}

fn latent_type() -> ModuleType {
    ModuleType {
        descriptor: LATENT,
        create: || Primitive::StateOutput(Box::new(Latent::default())),
    }
}

fn registry() -> Registry {
    let mut registry = common::registry();
    registry.register(latent_type()).unwrap();
    registry
}

/// The frame counter into `lat`, `lat` into the recorder.
fn latent_plan(registry: &Registry) -> signal_graph::ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            inst("count", "test.frames"),
            inst("lat", "test.latent"),
            inst("rec", "test.recorder"),
        ],
        connections: vec![conn("count.out", "lat.in"), conn("lat.out", "rec.alpha")],
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, 1).unwrap()
}

/// What `lat` outputs at frame `n` when every due result is in.
fn expected(n: usize) -> f64 {
    let m = n / BLOCK;
    if m == 0 {
        return 0.0;
    }
    ((m - 1) * BLOCK..m * BLOCK).map(|j| j as f64).sum()
}

/// The recorder's `alpha` input (its second channel) of a render.
fn alpha(samples: &[f64]) -> Vec<f64> {
    samples.iter().skip(1).step_by(2).copied().collect()
}

#[test]
fn offline_waits_for_every_due_result_for_any_k_and_threads() {
    let registry = registry();
    let plan = latent_plan(&registry);
    let frames = 41;
    let mut renders = Vec::new();
    for (threads, k) in [(1, 256), (1, 1), (1, 3), (2, 5), (4, 256)] {
        let config = EngineConfig {
            threads,
            block_capacity: k,
            schedule: Schedule::FORCED,
            ..EngineConfig::new(48_000, 42)
        };
        let mut engine = Engine::new(&plan, &registry, config, Vec::new()).unwrap();
        let r = render(&mut engine, frames).unwrap();
        assert_eq!(engine.late_results(), 0, "{threads} threads, k = {k}");
        let want: Vec<f64> = (0..frames as usize).map(expected).collect();
        assert_eq!(alpha(&r.samples), want, "{threads} threads, k = {k}");
        assert!(!String::from_utf8_lossy(&r.log).contains("late_result"));
        renders.push(r);
    }
    for r in &renders[1..] {
        assert_eq!(r.samples, renders[0].samples);
        assert_eq!(r.log, renders[0].log);
    }
}

#[derive(Default)]
struct Faults {
    late: Vec<u64>,
    other: usize,
}

impl ControlSink for Faults {
    fn log(&mut self, line: LogLine<'_>) {
        match line {
            LogLine::Fault {
                frame,
                reason: FaultReason::LateResult,
                address,
                port,
                // Merely late: no cause.
                cause: None,
            } => {
                assert_eq!(address.to_string(), "/lat");
                assert_eq!(port, Some(&port_addr("/lat.out")));
                self.late.push(frame.0);
            }
            LogLine::Fault { .. } => self.other += 1,
            _ => {}
        }
    }
}

/// In real time the engine asks once and gives up: every due block is a
/// `late_result` fault at its first frame, counted by the engine and the
/// runner; the output is silence, and the instance is not muted (no other
/// fault, and the next block asks again).
#[test]
fn real_time_never_waits_and_counts_each_missed_result() {
    let registry = registry();
    let plan = latent_plan(&registry);
    let mut config = RealtimeConfig::with_buffer(48_000, 42, 64);
    config.engine.threads = 2;
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let mut driver = NullDriver::new(48_000, 2, 3);
    driver.start(runner).unwrap();
    let mut faults = Faults::default();
    let frames = 1_000;
    driver.run(frames, || controller.poll(&mut faults)).unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut faults);

    let due: Vec<u64> = (1..frames.div_ceil(BLOCK as u64))
        .map(|m| m * BLOCK as u64)
        .collect();
    assert_eq!(faults.late, due);
    assert_eq!(faults.other, 0);
    assert_eq!(runner.status().late_results, due.len() as u64);
    assert!(driver.output().iter().all(|&x| x == 0.0));
}

#[test]
fn the_registry_refuses_a_bad_latency() {
    let bad = |descriptor: ModuleDescriptor| {
        let mut registry = Registry::new();
        registry.register(ModuleType {
            descriptor,
            ..latent_type()
        })
    };
    let latency = |latency_blocks, block_frames| {
        Some(BlockLatency {
            latency_blocks,
            block_frames,
        })
    };
    for descriptor in [
        ModuleDescriptor {
            feedthrough: Feedthrough::Direct,
            ..LATENT
        },
        ModuleDescriptor {
            latency: latency(0, 4),
            ..LATENT
        },
        ModuleDescriptor {
            latency: latency(1, 0),
            ..LATENT
        },
    ] {
        assert_eq!(
            bad(descriptor),
            Err(RegistryError::InvalidLatency("test.latent"))
        );
    }
    assert_eq!(bad(LATENT), Ok(()));
}

/// The manifest names each `same_machine` instance with its latency; a
/// render without one keeps the manifest it always had (no key).
#[test]
fn the_manifest_records_same_machine_instances() {
    let registry = registry();
    let mut engine = Engine::new(
        &latent_plan(&registry),
        &registry,
        EngineConfig::new(48_000, 42),
        Vec::new(),
    )
    .unwrap();
    let r = render(&mut engine, 16).unwrap();
    let recorded = [SameMachine {
        instance: "/lat".to_owned(),
        type_name: "test.latent".to_owned(),
        latency_blocks: Some(1),
        block_frames: Some(BLOCK as u32),
    }];
    assert_eq!(r.same_machine, recorded);
    let json = serde_json::to_value(r.manifest(Default::default())).unwrap();
    assert_eq!(
        json["same_machine"],
        serde_json::json!([{
            "instance": "/lat", "type": "test.latent", "latency_blocks": 1, "block_frames": 4,
        }])
    );

    let exact = plan(
        vec![inst("count", "test.frames"), inst("rec", "test.recorder")],
        vec![conn("count.out", "rec.alpha")],
    );
    let mut engine = common::engine(&exact, 48_000, Vec::new());
    let r = render(&mut engine, 16).unwrap();
    assert!(r.same_machine.is_empty());
    let json = serde_json::to_value(r.manifest(Default::default())).unwrap();
    assert!(json.get("same_machine").is_none(), "{json}");
}

// M2.7-4: boundary outcomes. `test.mixed_latent` (tests/common) delivers its
// even blocks and misses its odd ones in real time.

/// The frame counter into `lat`; `lat.out` into the recorder's `alpha`, and
/// `lat.<second>` (`polls` or `settles`) into its `zeta`.
fn mixed_plan(registry: &Registry, second: &str) -> signal_graph::ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            inst("count", "test.frames"),
            inst("lat", "test.mixed_latent"),
            inst("rec", "test.recorder"),
        ],
        connections: vec![
            conn("count.out", "lat.in"),
            conn("lat.out", "rec.alpha"),
            conn(&format!("lat.{second}"), "rec.zeta"),
        ],
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, 1).unwrap()
}

/// The recorder's `zeta` input (its first channel) of a render.
fn zeta(samples: &[f64]) -> Vec<f64> {
    samples.iter().step_by(2).copied().collect()
}

/// The recorded outcome of `lat`'s due block m (frames 4m …): missed for
/// m ≡ 0 (mod 3), delivered and awaited for m ≡ 1, delivered with the
/// recorded word 42 + m for m ≡ 2.
fn script(m: u64) -> Scripted {
    match m % 3 {
        0 => Scripted::Missed(None),
        1 => Scripted::Delivered(None),
        _ => Scripted::Delivered(Some(vec![42.0 + m as f32].into_boxed_slice())),
    }
}

fn scripted_config(threads: usize, k: usize) -> EngineConfig {
    EngineConfig {
        threads,
        block_capacity: k,
        schedule: Schedule::FORCED,
        scripted_boundaries: true,
        ..EngineConfig::new(48_000, 42)
    }
}

/// An exact replay applies each pushed outcome at its due frame: a miss
/// without waiting (`due` asked once, then `miss`, a `late_result` fault),
/// a delivery with data by showing the data, one without by waiting; the
/// same for every *k* and thread count.
#[test]
fn exact_replay_applies_the_pushed_outcomes_for_any_k_and_threads() {
    let registry = common::latent_registry();
    let plan = mixed_plan(&registry, "polls");
    let frames = 41u64;
    let mut renders = Vec::new();
    for (threads, k) in [(1, 256), (1, 1), (1, 3), (2, 5), (4, 256)] {
        let mut engine =
            Engine::new(&plan, &registry, scripted_config(threads, k), Vec::new()).unwrap();
        for m in 1..=frames / 4 {
            engine
                .push_boundary("/lat".parse().unwrap(), Frame(4 * m), script(m))
                .unwrap();
        }
        let r = render(&mut engine, frames).unwrap();
        assert_eq!(
            engine.boundary_mismatches(),
            0,
            "{threads} threads, k = {k}"
        );
        assert_eq!(engine.boundary_unreproduced(), 0);
        assert_eq!(
            engine.late_results(),
            (1..=frames / 4).filter(|m| m.is_multiple_of(3)).count() as u64
        );
        let (out, polls) = (alpha(&r.samples), zeta(&r.samples));
        for (n, (&y, &p)) in out.iter().zip(&polls).enumerate() {
            let m = (n / 4) as u64;
            let want = match m {
                0 => 0.0,
                m if m.is_multiple_of(3) => 0.0,
                m if m % 3 == 1 => expected(n),
                m => f64::from(42.0 + m as f32),
            };
            assert_eq!(y, want, "frame {n}, {threads} threads, k = {k}");
            if m > 0 && m.is_multiple_of(3) && n.is_multiple_of(4) {
                assert_eq!(p, 1.0, "a recorded miss asks `due` once (frame {n})");
            }
        }
        renders.push(r);
    }
    for r in &renders[1..] {
        assert_eq!(r.samples, renders[0].samples);
        assert_eq!(r.log, renders[0].log);
    }
}

/// An outcome pushed for a frame that is not due, and a due frame without
/// one, are mismatches; the replay still runs.
#[test]
fn exact_replay_counts_mismatched_outcomes() {
    let registry = common::latent_registry();
    let plan = mixed_plan(&registry, "polls");
    let mut engine = Engine::new(&plan, &registry, scripted_config(1, 256), Vec::new()).unwrap();
    let lat: InstanceAddress = "/lat".parse().unwrap();
    for m in [1u64, 3, 4] {
        engine
            .push_boundary(lat.clone(), Frame(4 * m), script(m))
            .unwrap();
    }
    // Not due: the second frame of block 2, whose own outcome is not pushed.
    engine
        .push_boundary(lat, Frame(9), Scripted::Missed(None))
        .unwrap();
    render(&mut engine, 20).unwrap();
    assert_eq!(engine.boundary_mismatches(), 2);
}

/// `settle` is asked before each `absorb` in an exact replay only.
#[test]
fn settle_is_asked_in_exact_replay_only() {
    let registry = common::latent_registry();
    let plan = mixed_plan(&registry, "settles");
    let mut ideal =
        Engine::new(&plan, &registry, EngineConfig::new(48_000, 42), Vec::new()).unwrap();
    let r = render(&mut ideal, 16).unwrap();
    assert!(zeta(&r.samples).iter().all(|&s| s == 0.0));
    let mut exact = Engine::new(&plan, &registry, scripted_config(1, 256), Vec::new()).unwrap();
    for m in 1..4u64 {
        exact
            .push_boundary(
                "/lat".parse().unwrap(),
                Frame(4 * m),
                Scripted::Delivered(None),
            )
            .unwrap();
    }
    let r = render(&mut exact, 16).unwrap();
    assert_eq!(
        zeta(&r.samples).last(),
        Some(&15.0),
        "one per absorb before the last frame's emit"
    );
}

/// What the Controller hands to `boundary`, and the last status.
#[derive(Default)]
struct Outcomes {
    delivered: Vec<(u64, Option<Vec<f32>>)>,
    missed: Vec<u64>,
    status: Option<signal_engine::RunnerStatus>,
}

impl ControlSink for Outcomes {
    fn boundary(&mut self, line: BoundaryLine<'_>) {
        assert_eq!(line.address.to_string(), "/lat");
        match line.outcome {
            BoundaryOutcome::Delivered { data, data_lost } => {
                assert!(!data_lost);
                self.delivered
                    .push((line.frame.0, data.map(<[f32]>::to_vec)));
            }
            BoundaryOutcome::Missed { cause } => {
                assert_eq!(cause, None);
                self.missed.push(line.frame.0);
            }
        }
    }
    fn status(&mut self, status: &signal_engine::RunnerStatus) {
        self.status = Some(*status);
    }
}

/// C1/C2/C3 in the engine: in real time every due block is a delivered or a
/// missed outcome at its first frame (even blocks delivered, odd missed),
/// counted by the runner; with `record_boundary_data` each delivery carries
/// its result, through the boundary-data queue.
#[test]
fn real_time_reports_every_due_block_with_its_data() {
    let registry = common::latent_registry();
    let plan = mixed_plan(&registry, "polls");
    let mut config = RealtimeConfig::with_buffer(48_000, 42, 64);
    config.engine.threads = 2;
    config.engine.record_boundary_data = true;
    config.boundary_data = 1024;
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let mut driver = NullDriver::new(48_000, 2, 3);
    driver.start(runner).unwrap();
    let mut outcomes = Outcomes::default();
    let frames = 1_000;
    driver
        .run(frames, || controller.poll(&mut outcomes))
        .unwrap();
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut outcomes);

    let blocks = 1..frames.div_ceil(MIXED_BLOCK as u64);
    let even: Vec<(u64, Option<Vec<f32>>)> = blocks
        .clone()
        .filter(|m| m.is_multiple_of(2))
        .map(|m| {
            let n = (m * MIXED_BLOCK as u64) as usize;
            (n as u64, Some(vec![expected(n) as f32]))
        })
        .collect();
    let odd: Vec<u64> = blocks
        .filter(|m| m % 2 == 1)
        .map(|m| m * MIXED_BLOCK as u64)
        .collect();
    assert_eq!(outcomes.delivered, even);
    assert_eq!(outcomes.missed, odd);
    let status = runner.status();
    assert_eq!(status.delivered_results, even.len() as u64);
    assert_eq!(status.late_results, odd.len() as u64);
    assert_eq!(status.boundary_data_overflows, 0);
    assert_eq!(
        outcomes.status.map(|s| s.next_frame),
        Some(status.next_frame)
    );
}

// M2.7-5: the cause of a lost result. `test.lossy` (like `test.latent`, one
// block of 4) loses its odd blocks' results with the cause `mapping`, and
// forgets the cause in `miss`, as `lab.gpu_partials` does: a cause read
// after `miss` would be none.

const LOSSY: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.lossy",
    ..LATENT
};

#[derive(Default)]
struct Lossy {
    frame: usize,
    block: u64,
    resolved: bool,
    cause: Option<LossCause>,
}

impl StateOutputPrimitive for Lossy {
    fn describe() -> ModuleDescriptor {
        LOSSY
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        *self = Lossy::default();
    }
    fn reset(&mut self) {
        *self = Lossy::default();
    }
    fn due(&mut self) -> Due {
        self.cause = None;
        if self.frame != 0 || self.resolved || self.block.is_multiple_of(2) {
            self.resolved = true;
            return Due::Ready;
        }
        self.cause = Some(LossCause::Mapping);
        Due::Lost
    }
    fn miss(&mut self) {
        self.resolved = true;
        self.cause = None;
    }
    fn lost_cause(&self) -> Option<LossCause> {
        self.cause
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(1.0);
    }
    fn absorb(&mut self, _inputs: &Inputs<'_>) {
        self.frame += 1;
        if self.frame == BLOCK {
            self.frame = 0;
            self.block += 1;
            self.resolved = false;
        }
    }
}

fn lossy_registry() -> Registry {
    let mut registry = common::registry();
    registry
        .register(ModuleType {
            descriptor: LOSSY,
            create: || Primitive::StateOutput(Box::new(Lossy::default())),
        })
        .unwrap();
    registry
}

fn lossy_plan(registry: &Registry) -> signal_graph::ExecutionPlan {
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            inst("count", "test.frames"),
            inst("lat", "test.lossy"),
            inst("rec", "test.recorder"),
        ],
        connections: vec![conn("count.out", "lat.in"), conn("lat.out", "rec.alpha")],
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, 1).unwrap()
}

/// The `late_result` lines of a render's log.
fn late_lines(log: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(log)
        .lines()
        .filter(|l| l.contains("late_result"))
        .map(str::to_owned)
        .collect()
}

/// The odd due frames below `frames`: the blocks `test.lossy` loses.
fn odd_due(frames: u64) -> Vec<u64> {
    (1..frames.div_ceil(BLOCK as u64))
        .filter(|m| m % 2 == 1)
        .map(|m| m * BLOCK as u64)
        .collect()
}

/// The cause of the `late_result` line at `frame`, as written: `none` when
/// the line has no cause; `None` when there is no such line.
fn cause_at(lines: &[String], frame: u64) -> Option<String> {
    let at = format!("\"frame\":{frame},");
    lines
        .iter()
        .find(|l| l.contains(&at))
        .map(|l| l.split("\"cause\":").nth(1).unwrap_or("none").to_owned())
}

/// Offline (`wait_due`): each lost result's fault line carries the cause
/// the module gave, read before `miss`.
#[test]
fn offline_a_lost_result_is_logged_with_its_cause() {
    let registry = lossy_registry();
    let mut engine = Engine::new(
        &lossy_plan(&registry),
        &registry,
        EngineConfig::new(48_000, 42),
        Vec::new(),
    )
    .unwrap();
    let r = render(&mut engine, 40).unwrap();
    let lines = late_lines(&r.log);
    assert_eq!(lines.len(), odd_due(40).len(), "{lines:#?}");
    for frame in odd_due(40) {
        assert_eq!(
            cause_at(&lines, frame).as_deref(),
            Some("\"mapping\"}"),
            "{lines:#?}"
        );
    }
}

#[derive(Default)]
struct Causes {
    faults: Vec<(u64, Option<LossCause>)>,
    missed: Vec<(u64, Option<LossCause>)>,
}

impl ControlSink for Causes {
    fn log(&mut self, line: LogLine<'_>) {
        if let LogLine::Fault {
            frame,
            reason: FaultReason::LateResult,
            cause,
            ..
        } = line
        {
            self.faults.push((frame.0, cause));
        }
    }
    fn boundary(&mut self, line: BoundaryLine<'_>) {
        if let BoundaryOutcome::Missed { cause } = line.outcome {
            self.missed.push((line.frame.0, cause));
        }
    }
}

/// Real time (`due` once): the same cause on the fault line and on the
/// missed boundary outcome a session records.
#[test]
fn real_time_a_lost_result_is_logged_with_its_cause() {
    let registry = lossy_registry();
    let plan = lossy_plan(&registry);
    let mut config = RealtimeConfig::with_buffer(48_000, 42, 64);
    config.engine.threads = 2;
    let (runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let mut driver = NullDriver::new(48_000, 2, 3);
    driver.start(runner).unwrap();
    let mut causes = Causes::default();
    let frames = 1_000;
    driver.run(frames, || controller.poll(&mut causes)).unwrap();
    driver.stop().unwrap().unwrap();
    controller.poll(&mut causes);
    let want: Vec<(u64, Option<LossCause>)> = odd_due(frames)
        .into_iter()
        .map(|f| (f, Some(LossCause::Mapping)))
        .collect();
    assert_eq!(causes.faults, want);
    assert_eq!(causes.missed, want);
}

/// Exact replay: a recorded miss is logged with the recorded cause (not the
/// module's), and a recorded delivery the module now loses is logged with
/// the module's cause.
#[test]
fn exact_replay_logs_the_recorded_cause() {
    let registry = lossy_registry();
    let config = EngineConfig {
        scripted_boundaries: true,
        ..EngineConfig::new(48_000, 42)
    };
    let mut engine = Engine::new(&lossy_plan(&registry), &registry, config, Vec::new()).unwrap();
    let lat: InstanceAddress = "/lat".parse().unwrap();
    for m in 1..10u64 {
        let outcome = match m {
            1 => Scripted::Missed(Some(LossCause::DeviceLost)),
            2 => Scripted::Missed(None),
            _ => Scripted::Delivered(None),
        };
        engine
            .push_boundary(lat.clone(), Frame(4 * m), outcome)
            .unwrap();
    }
    let r = render(&mut engine, 40).unwrap();
    assert_eq!(engine.boundary_mismatches(), 0);
    let lines = late_lines(&r.log);
    assert_eq!(cause_at(&lines, 4).as_deref(), Some("\"device_lost\"}"));
    assert_eq!(cause_at(&lines, 8).as_deref(), Some("none"));
    assert_eq!(cause_at(&lines, 12).as_deref(), Some("\"mapping\"}"));
    assert_eq!(cause_at(&lines, 16), None, "an even block is delivered");
}
