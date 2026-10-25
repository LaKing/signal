//! The suite tests itself: the correct fixtures pass every check through
//! `contract_tests!`, and each broken fixture fails exactly the check it targets.

use signal_contract::{Case, Check, Failure, Settings, stimulus};
use signal_core::{
    Combine, DirectPrimitive, Direction, EventPayload, Feedthrough, Inputs, ModuleDescriptor,
    ModuleType, Outputs, ParameterDescriptor, PortDescriptor, PortKind, PortShape, PrepareContext,
    Primitive, Range, RateDivisor, StateOutputPrimitive,
};

signal_contract::install_counting_allocator!();

const fn port(name: &'static str, direction: Direction, shape: PortShape) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape,
        rate: RateDivisor::ONE,
        kind: PortKind::Generic,
        unit: None,
        range: None,
    }
}

const IN: PortDescriptor = port("in", Direction::In, PortShape::Signal);
const OUT: PortDescriptor = port("out", Direction::Out, PortShape::Signal);
const EVENTS_IN: PortDescriptor = port("events", Direction::In, PortShape::Events);
const EVENTS_OUT: PortDescriptor = port("events_out", Direction::Out, PortShape::Events);

const fn descriptor(
    type_name: &'static str,
    ports: &'static [PortDescriptor],
    parameters: &'static [ParameterDescriptor],
    feedthrough: Feedthrough,
) -> ModuleDescriptor {
    ModuleDescriptor {
        type_name,
        doc: "contract suite fixture",
        ports,
        parameters,
        feedthrough,
        sink: None,
        latency: None,
        reproducibility: signal_core::Reproducibility::BitExact,
        category: None,
    }
}

fn copy_input(inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
    outputs.signal(0).copy_from_slice(inputs.signal(0));
}

// ---- Correct fixtures ------------------------------------------------------

/// `out = in * gain`; forwards every input event unchanged.
struct Gain;

impl DirectPrimitive for Gain {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.gain",
            &[IN, EVENTS_IN, OUT, EVENTS_OUT],
            &[ParameterDescriptor {
                name: "gain",
                kind: PortKind::Gain,
                rate: RateDivisor::ONE,
                unit: None,
                range: Range { min: 0.0, max: 4.0 },
                default: 1.0,
                combine: Combine::Multiply,
                values: None,
                snap: signal_core::Snap::Nearest,
            }],
            Feedthrough::Direct,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let samples = inputs.signal(0).iter().zip(inputs.parameter(0));
        for (out, (x, gain)) in outputs.signal(0).iter_mut().zip(samples) {
            *out = x * gain;
        }
        if let Some(sink) = outputs.events(0) {
            for &event in inputs.events(0) {
                sink.push(event);
            }
        }
    }
}

fn gain_type() -> ModuleType {
    ModuleType {
        descriptor: Gain::describe(),
        create: || Primitive::Direct(Box::new(Gain)),
    }
}

/// `out[n] = in[n - 1]`, `out[0] = 0`.
#[derive(Default)]
struct UnitDelay {
    last: f64,
}

impl StateOutputPrimitive for UnitDelay {
    fn describe() -> ModuleDescriptor {
        descriptor("test.unit_delay", &[IN, OUT], &[], Feedthrough::StateOutput)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.last = 0.0;
    }

    fn reset(&mut self) {
        self.last = 0.0;
    }

    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(self.last);
    }

    fn absorb(&mut self, inputs: &Inputs<'_>) {
        if let Some(&x) = inputs.signal(0).last() {
            self.last = x;
        }
    }
}

fn unit_delay_type() -> ModuleType {
    ModuleType {
        descriptor: UnitDelay::describe(),
        create: || Primitive::StateOutput(Box::<UnitDelay>::default()),
    }
}

mod gain {
    signal_contract::contract_tests!(super::gain_type());
}

mod unit_delay {
    signal_contract::contract_tests!(super::unit_delay_type());
}

// ---- Broken fixtures, one per check ----------------------------------------

/// Copies its input through a temporary `Vec`: allocates and frees in `process`.
struct AllocatesInProcess;

impl DirectPrimitive for AllocatesInProcess {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.allocates_in_process",
            &[IN, OUT],
            &[],
            Feedthrough::Direct,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let scratch: Vec<f64> = inputs.signal(0).to_vec();
        outputs.signal(0).copy_from_slice(&scratch);
    }
}

/// Passes its input through and emits a trigger at the start of every call:
/// "once per callback", so its events depend on *k*.
struct TriggerPerCall;

impl DirectPrimitive for TriggerPerCall {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.trigger_per_call",
            &[IN, OUT, EVENTS_OUT],
            &[],
            Feedthrough::Direct,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        copy_input(inputs, outputs);
        if let Some(sink) = outputs.events(0) {
            sink.push(signal_core::Event {
                offset: 0,
                payload: EventPayload::Trigger,
            });
        }
    }
}

/// `out = 1 / p` with `p` in `[0, 1]`: infinite at the range endpoint 0.
struct Reciprocal;

impl DirectPrimitive for Reciprocal {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.reciprocal",
            &[OUT],
            &[ParameterDescriptor {
                name: "p",
                kind: PortKind::Control,
                rate: RateDivisor::ONE,
                unit: None,
                range: Range { min: 0.0, max: 1.0 },
                default: 1.0,
                combine: Combine::Add,
                values: None,
                snap: signal_core::Snap::Nearest,
            }],
            Feedthrough::Direct,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        for (out, p) in outputs.signal(0).iter_mut().zip(inputs.parameter(0)) {
            *out = 1.0 / p;
        }
    }
}

/// Declares itself state-output but is created as a direct instance whose
/// output is its same-frame input. A `StateOutputPrimitive` cannot read
/// same-frame input (`emit` gets no inputs), so a type can break ADR 0016 §2
/// only by declaring a class its instances do not honour.
struct ClaimsStateOutput;

impl DirectPrimitive for ClaimsStateOutput {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.claims_state_output",
            &[IN, OUT],
            &[],
            Feedthrough::StateOutput,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        copy_input(inputs, outputs);
    }
}

/// `state = state / 2 + in`, `out = state`, with no denormal flush: the decay
/// after an impulse passes through subnormal values.
#[derive(Default)]
struct SubnormalTail {
    state: f64,
}

impl DirectPrimitive for SubnormalTail {
    fn describe() -> ModuleDescriptor {
        descriptor("test.subnormal_tail", &[IN, OUT], &[], Feedthrough::Direct)
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.state = 0.0;
    }

    fn reset(&mut self) {
        self.state = 0.0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        for (out, x) in outputs.signal(0).iter_mut().zip(inputs.signal(0)) {
            self.state = self.state * 0.5 + x;
            *out = self.state;
        }
    }
}

/// `out = in * 48000 / rate`, with the factor computed in the first `prepare`
/// only: preparing again at another rate keeps the stale factor.
#[derive(Default)]
struct StaleRate {
    factor: Option<f64>,
}

impl DirectPrimitive for StaleRate {
    fn describe() -> ModuleDescriptor {
        descriptor("test.stale_rate", &[IN, OUT], &[], Feedthrough::Direct)
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.factor.get_or_insert(48_000.0 / ctx.sample_rate);
    }

    fn reset(&mut self) {}

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let factor = self.factor.unwrap_or(1.0);
        for (out, x) in outputs.signal(0).iter_mut().zip(inputs.signal(0)) {
            *out = x * factor;
        }
    }
}

/// `SubnormalTail` declared with an input range of `[-1, 0]`, where clamping 1
/// into the range gives the silence value itself (auditor-1-001).
fn negative_subnormal_tail_type() -> ModuleType {
    const NEGATIVE_IN: PortDescriptor = PortDescriptor {
        range: Some(Range {
            min: -1.0,
            max: 0.0,
        }),
        ..IN
    };
    ModuleType {
        descriptor: descriptor(
            "test.negative_subnormal_tail",
            &[NEGATIVE_IN, OUT],
            &[],
            Feedthrough::Direct,
        ),
        create: || Primitive::Direct(Box::<SubnormalTail>::default()),
    }
}

/// Passes its input through and panics when it reaches frame 7.
#[derive(Default)]
struct PanicsAtFrame7 {
    frame: u64,
}

impl DirectPrimitive for PanicsAtFrame7 {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.panics_at_frame_7",
            &[IN, OUT],
            &[],
            Feedthrough::Direct,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.frame = 0;
    }

    fn reset(&mut self) {
        self.frame = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        for (out, x) in outputs.signal(0).iter_mut().zip(inputs.signal(0)) {
            assert_ne!(self.frame, 7, "frame seven");
            *out = *x;
            self.frame += 1;
        }
    }
}

/// A state-output primitive whose `emit` pushes events until its sink refuses
/// one, so every call overflows.
struct FloodsEvents;

impl StateOutputPrimitive for FloodsEvents {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.floods_events",
            &[IN, OUT, EVENTS_OUT],
            &[],
            Feedthrough::StateOutput,
        )
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {}

    fn reset(&mut self) {}

    fn emit(&self, outputs: &mut Outputs<'_>) {
        outputs.signal(0).fill(0.0);
        if let Some(sink) = outputs.events(0) {
            let trigger = signal_core::Event {
                offset: 0,
                payload: EventPayload::Trigger,
            };
            while sink.push(trigger) {}
        }
    }

    fn absorb(&mut self, _inputs: &Inputs<'_>) {}
}

// ---- M2.8-6: divided rates and vectors -------------------------------------

/// `port` at rate divisor `d`.
const fn at(d: u32, port: PortDescriptor) -> PortDescriptor {
    PortDescriptor {
        rate: match RateDivisor::new(d) {
            Some(rate) => rate,
            None => RateDivisor::ONE,
        },
        ..port
    }
}

/// A gain in [0, 4] at divisor 1.
const GAIN_PARAMETER: ParameterDescriptor = ParameterDescriptor {
    name: "gain",
    kind: PortKind::Gain,
    rate: RateDivisor::ONE,
    unit: None,
    range: Range { min: 0.0, max: 4.0 },
    default: 1.0,
    combine: Combine::Multiply,
    values: None,
    snap: signal_core::Snap::Nearest,
};

const V4_IN: PortDescriptor = port("in", Direction::In, PortShape::Vector(4));
const V4_OUT: PortDescriptor = port("out", Direction::Out, PortShape::Vector(4));
const VECTOR_PORTS: &[PortDescriptor] = &[at(64, V4_IN), at(64, V4_OUT)];
const VECTOR_GAIN: ParameterDescriptor = ParameterDescriptor {
    rate: match RateDivisor::new(64) {
        Some(rate) => rate,
        None => RateDivisor::ONE,
    },
    ..GAIN_PARAMETER
};

/// A vector of 4 at divisor 64: element `e` is `in[e] · gain · (e + 1)`. It
/// sees ticks only; the suite calls it with ticks, as the engine does.
struct VectorGain;

impl DirectPrimitive for VectorGain {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.vector_gain",
            VECTOR_PORTS,
            &[VECTOR_GAIN],
            Feedthrough::Direct,
        )
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let gain = inputs.parameter(0);
        for (e, row) in outputs.signals().enumerate() {
            for (out, (x, g)) in row.iter_mut().zip(inputs.signal(e).iter().zip(gain)) {
                *out = x * g * (e + 1) as f64;
            }
        }
    }
}

/// A unit delay per element of a vector of 4, at divisor 3 (state-output).
#[derive(Default)]
struct VectorDelay {
    last: [f64; 4],
}

impl StateOutputPrimitive for VectorDelay {
    fn describe() -> ModuleDescriptor {
        const PORTS: &[PortDescriptor] = &[at(3, V4_IN), at(3, V4_OUT)];
        descriptor("test.vector_delay", PORTS, &[], Feedthrough::StateOutput)
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.last = [0.0; 4];
    }
    fn reset(&mut self) {
        self.last = [0.0; 4];
    }
    fn emit(&self, outputs: &mut Outputs<'_>) {
        for (row, &x) in outputs.signals().zip(&self.last) {
            row.fill(x);
        }
    }
    fn absorb(&mut self, inputs: &Inputs<'_>) {
        for (e, last) in self.last.iter_mut().enumerate() {
            if let Some(&x) = inputs.signal(e).last() {
                *last = x;
            }
        }
    }
}

fn vector_gain_type() -> ModuleType {
    ModuleType {
        descriptor: VectorGain::describe(),
        create: || Primitive::Direct(Box::new(VectorGain)),
    }
}

fn vector_delay_type() -> ModuleType {
    ModuleType {
        descriptor: VectorDelay::describe(),
        create: || Primitive::StateOutput(Box::<VectorDelay>::default()),
    }
}

mod vector_gain {
    signal_contract::contract_tests!(super::vector_gain_type());
}

mod vector_delay {
    signal_contract::contract_tests!(super::vector_delay_type());
}

/// A vector of 4 at divisor 64 whose element 1 is the number of ticks in
/// the call: it depends on how ticks are split into calls.
struct VectorPerCall;

impl DirectPrimitive for VectorPerCall {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.vector_per_call",
            VECTOR_PORTS,
            &[],
            Feedthrough::Direct,
        )
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        let ticks = inputs.frames() as f64;
        outputs.rows(0, 4).copy_from_slice(inputs.rows(0, 4));
        outputs.signal(1).fill(ticks);
    }
}

/// A vector of 4 that never writes its last element.
struct VectorUnwritten;

impl DirectPrimitive for VectorUnwritten {
    fn describe() -> ModuleDescriptor {
        descriptor(
            "test.vector_unwritten",
            VECTOR_PORTS,
            &[],
            Feedthrough::Direct,
        )
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        outputs.rows(0, 3).copy_from_slice(inputs.rows(0, 3));
    }
}

#[test]
fn a_vector_that_depends_on_its_call_size_fails_partition_invariance() {
    let failure = fails_only(
        ModuleType {
            descriptor: VectorPerCall::describe(),
            create: || Primitive::Direct(Box::new(VectorPerCall)),
        },
        Check::PartitionInvariance,
    );
    assert!(
        failure.detail.contains("signal output `out[1]`"),
        "{failure}"
    );
}

#[test]
fn an_unwritten_vector_element_fails_finite_output() {
    let failure = fails_only(
        ModuleType {
            descriptor: VectorUnwritten::describe(),
            create: || Primitive::Direct(Box::new(VectorUnwritten)),
        },
        Check::FiniteOutput,
    );
    assert!(
        failure.detail.contains("signal output `out[3]`"),
        "{failure}"
    );
}

/// A type without one rate divisor (its output at 64, its parameter at 1)
/// makes every render invalid: every check fails, naming why.
#[test]
fn a_type_without_one_rate_fails_every_check() {
    const MIXED: &[PortDescriptor] = &[at(64, OUT)];
    let module_type = ModuleType {
        descriptor: descriptor(
            "test.mixed_rates",
            MIXED,
            &[GAIN_PARAMETER],
            Feedthrough::Direct,
        ),
        create: || Primitive::Direct(Box::new(VectorUnwritten)),
    };
    for check in Check::ALL {
        let report = check
            .run(
                &module_type,
                stimulus::random,
                &Settings::without_persistence(),
            )
            .expect_err("no one rate fails every check")
            .to_string();
        assert!(
            report.contains("no one rate divisor") && report.contains("test.mixed_rates"),
            "`{check}`: {report}"
        );
    }
}

/// Runs every check on `module_type` and asserts that exactly `expected` fails,
/// with a report naming the module type, the seed and a frame.
fn fails_only(module_type: ModuleType, expected: Check) -> Failure {
    let settings = Settings::without_persistence();
    let mut failure = None;
    for check in Check::ALL {
        match check.run(&module_type, stimulus::random, &settings) {
            Err(f) if check == expected => failure = Some(f),
            Err(f) => panic!("`{check}` should pass but failed: {f}"),
            Ok(()) if check == expected => panic!("`{check}` should fail but passed"),
            Ok(()) => {}
        }
    }
    let failure = failure.expect("the expected check failed");
    let report = failure.to_string();
    eprintln!("{report}");
    assert_eq!(failure.check, expected);
    for needle in [module_type.descriptor.type_name, "seed", "frame"] {
        assert!(report.contains(needle), "report lacks `{needle}`: {report}");
    }
    failure
}

#[test]
fn allocates_in_process_fails_no_allocation() {
    fails_only(
        ModuleType {
            descriptor: AllocatesInProcess::describe(),
            create: || Primitive::Direct(Box::new(AllocatesInProcess)),
        },
        Check::NoAllocation,
    );
}

#[test]
fn depends_on_k_fails_partition_invariance() {
    let module_type = ModuleType {
        descriptor: TriggerPerCall::describe(),
        create: || Primitive::Direct(Box::new(TriggerPerCall)),
    };
    let failure = fails_only(module_type, Check::PartitionInvariance);
    assert!(
        failure.detail.contains("event output `events_out`"),
        "{failure}"
    );

    // A printed case replays without proptest: calls of 3 frames differ from
    // k = 1, calls of 1 frame do not.
    let replay = |blocks| {
        let case = Case { seed: 7, blocks };
        Check::PartitionInvariance.run_case(&module_type, stimulus::random, &case)
    };
    let report = replay(vec![3]).expect_err("k = 3 differs").to_string();
    assert!(report.contains("first difference at frame 1"), "{report}");
    assert!(replay(vec![1]).is_ok());
}

#[test]
fn non_finite_output_fails_finite_output() {
    fails_only(
        ModuleType {
            descriptor: Reciprocal::describe(),
            create: || Primitive::Direct(Box::new(Reciprocal)),
        },
        Check::FiniteOutput,
    );
}

#[test]
fn reads_same_frame_input_fails_state_output_honesty() {
    let failure = fails_only(
        ModuleType {
            descriptor: ClaimsStateOutput::describe(),
            create: || Primitive::Direct(Box::new(ClaimsStateOutput)),
        },
        Check::StateOutputHonesty,
    );
    assert!(
        failure.detail.contains("returns a direct instance"),
        "{failure}"
    );
}

#[test]
fn subnormal_tail_fails_no_subnormals() {
    fails_only(
        ModuleType {
            descriptor: SubnormalTail::describe(),
            create: || Primitive::Direct(Box::<SubnormalTail>::default()),
        },
        Check::NoSubnormals,
    );
}

#[test]
fn stale_rate_fails_rate_agnostic() {
    fails_only(
        ModuleType {
            descriptor: StaleRate::describe(),
            create: || Primitive::Direct(Box::<StaleRate>::default()),
        },
        Check::RateAgnostic,
    );
}

#[test]
fn one_sided_range_subnormal_tail_fails_no_subnormals() {
    let failure = fails_only(negative_subnormal_tail_type(), Check::NoSubnormals);
    assert!(failure.detail.contains("frame 1023"), "{failure}");
    assert!(
        failure.detail.contains("is -1.1125369292536007e-308"),
        "{failure}"
    );
}

#[test]
fn a_panic_is_reported_with_the_seed_and_its_frames() {
    let module_type = ModuleType {
        descriptor: PanicsAtFrame7::describe(),
        create: || Primitive::Direct(Box::<PanicsAtFrame7>::default()),
    };
    let report = Check::FiniteOutput
        .run(
            &module_type,
            stimulus::random,
            &Settings::without_persistence(),
        )
        .expect_err("the panic fails the check")
        .to_string();
    for needle in [
        "module type `test.panics_at_frame_7`",
        "seed",
        "`process` panicked in the call covering frames",
        "frame seven",
    ] {
        assert!(report.contains(needle), "report lacks `{needle}`: {report}");
    }

    // The frames are those of the call that panicked: frame 7 itself with
    // calls of one frame, the call 4..8 with calls of four.
    for (blocks, frames) in [(vec![1], "frames 7..8"), (vec![4], "frames 4..8")] {
        let case = Case { seed: 3, blocks };
        let report = Check::FiniteOutput
            .run_case(&module_type, stimulus::random, &case)
            .expect_err("the panic fails the check")
            .to_string();
        assert!(report.contains(frames), "report lacks `{frames}`: {report}");
        assert!(report.contains("seed 3"), "{report}");
    }
}

#[test]
fn an_event_sink_overflow_fails_every_check() {
    let module_type = ModuleType {
        descriptor: FloodsEvents::describe(),
        create: || Primitive::StateOutput(Box::new(FloodsEvents)),
    };
    for check in Check::ALL {
        let report = check
            .run(
                &module_type,
                stimulus::random,
                &Settings::without_persistence(),
            )
            .expect_err("an overflow fails every check")
            .to_string();
        assert!(
            report.contains("event output `events_out` overflowed its sink"),
            "`{check}`: {report}"
        );
    }
}

/// What a generated test prints when it fails.
#[test]
#[should_panic(
    expected = "contract check `partition_invariance` failed for module type `test.trigger_per_call`"
)]
fn a_failing_check_panics_with_its_report() {
    let module_type = ModuleType {
        descriptor: TriggerPerCall::describe(),
        create: || Primitive::Direct(Box::new(TriggerPerCall)),
    };
    Check::PartitionInvariance.assert(
        &module_type,
        stimulus::random,
        &Settings::without_persistence(),
    );
}
