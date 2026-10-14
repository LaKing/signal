//! The checks: each one property of the module contract, run under proptest
//! over random seeds and call sizes, at every rate in [`RATES`].

use std::fmt;

use proptest::collection::vec;
use proptest::prelude::{Strategy, any};
use proptest::test_runner::{
    Config, FailurePersistence, FileFailurePersistence, TestCaseError, TestError, TestRunner,
};
use signal_core::{Feedthrough, Lent, ModuleDescriptor, ModuleType, Primitive};

use crate::alloc_counter;
use crate::render::{Instance, Panic, Rendered, first_difference};
use crate::stimulus::{self, Stimulus, StimulusFn};

/// Every check prepares its instances at each of these rates, in frames per second.
pub const RATES: [f64; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];

/// Frames rendered per case.
const FRAMES: usize = 4096;
/// The largest call size drawn for a case.
const MAX_BLOCK: usize = 1024;
/// Frames whose inputs the state-output honesty check perturbs are below this.
const HONESTY_FRAMES: usize = 512;
/// Silence after the impulse in the subnormal check.
const TAIL_FRAMES: usize = 65_536;
/// Call size of the subnormal check.
const TAIL_BLOCK: usize = 256;
/// Cases per check unless `PROPTEST_CASES` says otherwise.
const DEFAULT_CASES: u32 = 32;

/// One property of the module contract. [`contract_tests!`](crate::contract_tests)
/// generates one `#[test]` per check, named [`Check::name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Check {
    /// Rendering with *k* = 1 and with random call sizes gives bit-identical
    /// outputs, including emitted events and their frames (ADR 0002).
    PartitionInvariance,
    /// Rendering after `prepare` and again after `reset` gives identical outputs.
    ResetDeterminism,
    /// Finite in-range inputs, including range endpoints and 0, give finite outputs.
    FiniteOutput,
    /// `process`, `emit`, `absorb` and `reset` neither allocate nor free.
    NoAllocation,
    /// For a module declared state-output, changing the inputs of frame *n*
    /// never changes the outputs of frame *n* (ADR 0016 §2).
    StateOutputHonesty,
    /// An instance prepared again at another rate renders exactly what a fresh
    /// instance prepared at that rate renders.
    RateAgnostic,
    /// After an impulse and long silence, no output is subnormal.
    NoSubnormals,
}

impl Check {
    /// Every check, in the order `contract_tests!` generates them.
    pub const ALL: [Check; 7] = [
        Check::PartitionInvariance,
        Check::ResetDeterminism,
        Check::FiniteOutput,
        Check::NoAllocation,
        Check::StateOutputHonesty,
        Check::RateAgnostic,
        Check::NoSubnormals,
    ];

    /// The check's name, which is also its test's name.
    pub fn name(self) -> &'static str {
        match self {
            Check::PartitionInvariance => "partition_invariance",
            Check::ResetDeterminism => "reset_determinism",
            Check::FiniteOutput => "finite_output",
            Check::NoAllocation => "no_allocation",
            Check::StateOutputHonesty => "state_output_honesty",
            Check::RateAgnostic => "rate_agnostic",
            Check::NoSubnormals => "no_subnormals",
        }
    }

    /// Runs the check on `module_type`: under proptest with `settings`, except
    /// for `no_subnormals`, whose stimulus is fixed (seed 0).
    pub fn run(
        self,
        module_type: &ModuleType,
        stimulus: StimulusFn,
        settings: &Settings,
    ) -> Result<(), Failure> {
        if self == Check::NoSubnormals {
            let case = Case {
                seed: 0,
                blocks: vec![TAIL_BLOCK],
            };
            return self
                .case(module_type, stimulus, &case, settings.lent.as_ref())
                .map_err(|detail| self.failure(module_type, detail));
        }
        let mut config = Config::default();
        if std::env::var_os("PROPTEST_CASES").is_none() {
            config.cases = settings.cases;
        }
        config.source_file = settings.regressions;
        config.failure_persistence = settings.regressions.map(|_| {
            Box::new(FileFailurePersistence::WithSource("proptest-regressions"))
                as Box<dyn FailurePersistence>
        });
        let cases = (any::<u64>().no_shrink(), vec(1..=MAX_BLOCK, 1..=16))
            .prop_map(|(seed, blocks)| Case { seed, blocks });
        let result = TestRunner::new(config).run(&cases, |case| {
            self.case(module_type, stimulus, &case, settings.lent.as_ref())
                .map_err(TestCaseError::fail)
        });
        let detail = match result {
            Ok(()) => return Ok(()),
            Err(TestError::Fail(reason, case)) => format!(
                "{reason}\nminimal failing case: seed {}, blocks {:?}; replay it with \
                 `Check::{self:?}.run_case(&module_type, stimulus, &Case {{ seed: {}, blocks: vec!{:?} }})`",
                case.seed, case.blocks, case.seed, case.blocks
            ),
            Err(TestError::Abort(reason)) => format!("proptest aborted: {reason}"),
        };
        Err(self.failure(module_type, detail))
    }

    /// Runs the check for one case, without proptest: how a printed failure is replayed.
    pub fn run_case(
        self,
        module_type: &ModuleType,
        stimulus: StimulusFn,
        case: &Case,
    ) -> Result<(), Failure> {
        self.case(module_type, stimulus, case, None)
            .map_err(|detail| self.failure(module_type, detail))
    }

    /// Runs the check and panics with the failure report if it fails.
    pub fn assert(self, module_type: &ModuleType, stimulus: StimulusFn, settings: &Settings) {
        if let Err(failure) = self.run(module_type, stimulus, settings) {
            panic!("{failure}");
        }
    }

    fn failure(self, module_type: &ModuleType, detail: String) -> Failure {
        Failure {
            check: self,
            module_type: module_type.descriptor.type_name,
            detail,
        }
    }

    fn case(
        self,
        module_type: &ModuleType,
        stimulus: StimulusFn,
        case: &Case,
        lent: Option<&Lent>,
    ) -> Result<(), String> {
        let descriptor = &module_type.descriptor;
        check_rate(descriptor)?;
        let names = output_names(descriptor);
        let seed = case.seed;
        let blocks: &[usize] = &case.blocks;
        let max_block = blocks.iter().copied().max().unwrap_or(1);
        match self {
            Check::PartitionInvariance => {
                let input = generate(descriptor, stimulus, seed, FRAMES)?;
                for rate in RATES {
                    let at = format!("at {rate} Hz, seed {seed}");
                    let one = Instance::new(module_type, lent, rate, 1, seed).render(
                        &input,
                        FRAMES,
                        &[1],
                    );
                    let split = Instance::new(module_type, lent, rate, max_block, seed)
                        .render(&input, FRAMES, blocks);
                    faults(&one, &names, &at)?;
                    faults(&split, &names, &at)?;
                    if let Some(d) = first_difference(&one, &split, &names) {
                        return Err(format!(
                            "{at}: k = 1 and blocks {blocks:?} differ; first difference at frame {}: {}",
                            d.frame, d.what
                        ));
                    }
                }
            }
            Check::ResetDeterminism => {
                let input = generate(descriptor, stimulus, seed, FRAMES)?;
                for rate in RATES {
                    let at = format!("at {rate} Hz, seed {seed}");
                    let mut instance = Instance::new(module_type, lent, rate, max_block, seed);
                    let first = instance.render(&input, FRAMES, blocks);
                    faults(&first, &names, &at)?;
                    instance.reset();
                    let again = instance.render(&input, FRAMES, blocks);
                    faults(&again, &names, &at)?;
                    if let Some(d) = first_difference(&first, &again, &names) {
                        return Err(format!(
                            "{at}: the render after `prepare` and the render after `reset` differ; \
                             first difference at frame {}: {}",
                            d.frame, d.what
                        ));
                    }
                }
            }
            Check::FiniteOutput => {
                let input = generate(descriptor, stimulus, seed, FRAMES)?;
                for rate in RATES {
                    let at = format!("at {rate} Hz, seed {seed}");
                    let rendered = Instance::new(module_type, lent, rate, max_block, seed)
                        .render(&input, FRAMES, blocks);
                    faults(&rendered, &names, &at)?;
                    if let Some(d) = rendered.first_value(&names, |x| !x.is_finite()) {
                        return Err(format!(
                            "{at}: non-finite output at frame {}: {} (NaN can also mean the sample was not written)",
                            d.frame, d.what
                        ));
                    }
                }
            }
            Check::NoAllocation => {
                if !alloc_counter::is_installed() {
                    return Err("the counting allocator is not installed in this test binary; \
                                add `signal_contract::install_counting_allocator!();` to the test file"
                        .to_owned());
                }
                let input = generate(descriptor, stimulus, seed, FRAMES)?;
                for rate in RATES {
                    let at = format!("at {rate} Hz, seed {seed}");
                    let mut instance = Instance::new(module_type, lent, rate, max_block, seed);
                    let rendered = instance.render(&input, FRAMES, blocks);
                    faults(&rendered, &names, &at)?;
                    if let Some((frame, call, counts)) = rendered.allocation {
                        return Err(format!(
                            "{at}: `{call}` touched the heap in the call starting at frame {frame} \
                             ({} allocations, {} frees)",
                            counts.allocations, counts.deallocations
                        ));
                    }
                    let counts = instance.reset();
                    if let Some(panic) = instance.fault() {
                        return Err(describe_panic(panic, &at));
                    }
                    if !counts.is_zero() {
                        return Err(format!(
                            "{at}: `reset` touched the heap after frame {FRAMES} ({} allocations, {} frees)",
                            counts.allocations, counts.deallocations
                        ));
                    }
                }
            }
            Check::StateOutputHonesty => {
                if descriptor.feedthrough != Feedthrough::StateOutput {
                    return Ok(());
                }
                let input = generate(descriptor, stimulus, seed, HONESTY_FRAMES)?;
                for rate in RATES {
                    let at = format!("at {rate} Hz, seed {seed}");
                    let baseline = Instance::new(module_type, lent, rate, 1, seed).render(
                        &input,
                        HONESTY_FRAMES,
                        &[1],
                    );
                    faults(&baseline, &names, &at)?;
                    for n in blocks
                        .iter()
                        .take(4)
                        .map(|b| b.saturating_sub(1) % HONESTY_FRAMES)
                    {
                        let changed = stimulus::perturb(descriptor, &input, n);
                        let mut instance = Instance::new(module_type, lent, rate, 1, seed);
                        let rendered = instance.render(&changed, n + 1, &[1]);
                        faults(&rendered, &names, &at)?;
                        if let Some(d) =
                            first_difference(&truncate(&baseline, n + 1), &rendered, &names)
                        {
                            let class = match (module_type.create)() {
                                Primitive::Direct(_) => {
                                    " (the descriptor declares state-output, but `create` returns a direct instance)"
                                }
                                Primitive::StateOutput(_) => "",
                            };
                            return Err(format!(
                                "{at}: changing the inputs of frame {n} changed the output; \
                                 first difference at frame {}: {}{class}",
                                d.frame, d.what
                            ));
                        }
                    }
                }
            }
            Check::RateAgnostic => {
                let input = generate(descriptor, stimulus, seed, FRAMES)?;
                for (i, rate) in RATES.into_iter().enumerate() {
                    let earlier = RATES[(i + RATES.len() - 1) % RATES.len()];
                    let at = format!("at {rate} Hz, seed {seed}");
                    let mut used = Instance::new(module_type, lent, earlier, max_block, seed);
                    let first = used.render(&input, FRAMES, blocks);
                    faults(&first, &names, &format!("at {earlier} Hz, seed {seed}"))?;
                    used.prepare(rate, max_block, seed);
                    let again = used.render(&input, FRAMES, blocks);
                    faults(&again, &names, &at)?;
                    let fresh = Instance::new(module_type, lent, rate, max_block, seed)
                        .render(&input, FRAMES, blocks);
                    faults(&fresh, &names, &at)?;
                    if let Some(d) = first_difference(&fresh, &again, &names) {
                        return Err(format!(
                            "{at}: an instance prepared again at {rate} Hz after running at {earlier} Hz \
                             differs from a fresh one; first difference at frame {}: {}",
                            d.frame, d.what
                        ));
                    }
                }
            }
            Check::NoSubnormals => {
                let input = stimulus::impulse(descriptor, TAIL_FRAMES);
                for rate in RATES {
                    let at = format!("at {rate} Hz, seed {seed}");
                    let rendered = Instance::new(module_type, lent, rate, max_block, seed).render(
                        &input,
                        TAIL_FRAMES,
                        blocks,
                    );
                    faults(&rendered, &names, &at)?;
                    if let Some(d) = rendered.first_value(&names, f64::is_subnormal) {
                        return Err(format!(
                            "{at}: after an impulse on frame 0 and silence, subnormal output at frame {}: {}",
                            d.frame, d.what
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One generated case: the stimulus (and random stream) seed, and the call
/// sizes a direct instance is given, repeated cyclically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    /// Seeds the stimulus generator and the instance's `RngStream`.
    pub seed: u64,
    /// Call sizes *k*, each ≥ 1. `state_output_honesty` perturbs frame
    /// `(k − 1) mod 512` for each of the first four.
    pub blocks: Vec<usize>,
}

/// How a check runs under proptest.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Cases per check, unless the `PROPTEST_CASES` environment variable is set.
    pub cases: u32,
    /// The test's source file (`file!()`): proptest records failing cases in
    /// `<file stem>.proptest-regressions` beside it and replays them first on
    /// the next run. `None` records nothing.
    pub regressions: Option<&'static str>,
    /// What every instance is lent in `prepare` (M2.7-2: a GPU device for a
    /// GPU-backed type), as the engine's host would; `None` lends nothing.
    pub lent: Option<Lent>,
}

impl Settings {
    /// Default cases, recording failures beside `source_file`.
    pub fn persisting(source_file: &'static str) -> Settings {
        Settings {
            cases: DEFAULT_CASES,
            regressions: Some(source_file),
            lent: None,
        }
    }

    /// Default cases, recording nothing (for tests that expect failures).
    pub fn without_persistence() -> Settings {
        Settings {
            cases: DEFAULT_CASES,
            regressions: None,
            lent: None,
        }
    }

    /// These settings, lending every instance `lent`.
    pub fn lending(self, lent: Lent) -> Settings {
        Settings {
            lent: Some(lent),
            ..self
        }
    }
}

/// A failed check: which check, which module type, and what went wrong
/// (rate, seed, frame of the first difference).
#[derive(Clone, Debug)]
pub struct Failure {
    /// The check that failed.
    pub check: Check,
    /// The module type's name.
    pub module_type: &'static str,
    /// The report: rate, seed, blocks and the first differing frame and output.
    pub detail: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "contract check `{}` failed for module type `{}`: {}",
            self.check, self.module_type, self.detail
        )
    }
}

impl std::error::Error for Failure {}

fn generate(
    descriptor: &ModuleDescriptor,
    stimulus: StimulusFn,
    seed: u64,
    frames: usize,
) -> Result<Stimulus, String> {
    let input = stimulus(descriptor, seed, frames);
    stimulus::check_shape(descriptor, &input, frames)
        .map_err(|problem| format!("invalid stimulus for seed {seed}: {problem}"))?;
    Ok(input)
}

/// The names of the signal output rows (a vector's elements as `port[e]`)
/// and of the event outputs.
fn output_names(descriptor: &ModuleDescriptor) -> (Vec<String>, Vec<String>) {
    use signal_core::{Direction, PortShape};
    let outputs = || {
        descriptor
            .ports
            .iter()
            .filter(|p| p.direction == Direction::Out)
    };
    let signals = outputs()
        .flat_map(|p| match p.shape {
            PortShape::Vector(n) => (0..n).map(|e| format!("{}[{e}]", p.name)).collect(),
            PortShape::Signal => vec![p.name.to_owned()],
            _ => Vec::new(),
        })
        .collect();
    let events = outputs()
        .filter(|p| p.shape == PortShape::Events)
        .map(|p| p.name.to_owned())
        .collect();
    (signals, events)
}

/// Whether the descriptor has one rate, and keeps a divided rate's limits
/// (M2.8-6, `docs/semantics.md` §16): what makes every render invalid.
fn check_rate(descriptor: &ModuleDescriptor) -> Result<(), String> {
    let Some(rate) = descriptor.rate() else {
        return Err(
            "the descriptor has no one rate divisor: its outputs, parameters and \
                    event ports must share one, and each signal input's must divide it or \
                    be a multiple of it"
                .to_owned(),
        );
    };
    let events = descriptor
        .ports
        .iter()
        .any(|p| p.shape == signal_core::PortShape::Events);
    if rate.get() > 1 && (events || descriptor.latency.is_some() || descriptor.sink.is_some()) {
        return Err(format!(
            "the descriptor is at rate divisor {} but has event ports, a declared latency \
             or a sink, which run at rate divisor 1 only",
            rate.get()
        ));
    }
    if descriptor
        .ports
        .iter()
        .any(|p| p.shape == signal_core::PortShape::Vector(0))
    {
        return Err("the descriptor has a vector port of 0 elements".to_owned());
    }
    Ok(())
}

/// Fails for what makes any render invalid, whatever the check: a panic in a
/// call of the primitive, or an event sink overflow.
fn faults(rendered: &Rendered, names: &(Vec<String>, Vec<String>), at: &str) -> Result<(), String> {
    if let Some(panic) = &rendered.panic {
        return Err(describe_panic(panic, at));
    }
    match rendered.overflow {
        Some((frame, port)) => {
            let name = names.1.get(port).map_or("?", String::as_str);
            Err(format!(
                "{at}: event output `{name}` overflowed its sink in the call starting at frame {frame}"
            ))
        }
        None => Ok(()),
    }
}

fn describe_panic(panic: &Panic, at: &str) -> String {
    let Panic {
        call,
        frames,
        message,
    } = panic;
    match frames {
        Some((start, end)) => format!(
            "{at}: `{call}` panicked in the call covering frames {start}..{end} \
             (first frame {start}): {message}"
        ),
        None => format!("{at}: `{call}` panicked before frame 0 of a render: {message}"),
    }
}

/// The part of `rendered` before frame `frames`.
fn truncate(rendered: &Rendered, frames: usize) -> Rendered {
    Rendered {
        signals: rendered
            .signals
            .iter()
            .map(|s| s.iter().copied().take(frames).collect())
            .collect(),
        events: rendered
            .events
            .iter()
            .map(|e| {
                e.iter()
                    .copied()
                    .filter(|&(f, _)| f < frames as u64)
                    .collect()
            })
            .collect(),
        ..Rendered::default()
    }
}
