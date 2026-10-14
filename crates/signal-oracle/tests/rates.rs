//! M2.8-6, AC1, AC6, AC7 (`docs/semantics.md` §16): graphs with rate
//! divisors {1, 2, 3, 64, 1600} and vector ports of {1, 4, 256} elements,
//! run by the oracle and by the block engine, which must match bit for bit
//! (audio, log, final parameters and instance states) for every block size,
//! partition and thread count.
//!
//! The oracle's graph is the spec itself, with its cross-rate connections:
//! the oracle inserts its converters from the rule in §16. The engine's
//! plans come from `signal_graph::build_plan`, which inserts its own. The
//! types are the fixtures of `common::rates`.
//!
//! - `the_block_engine_matches_the_oracle_with_rates_and_vectors`: generated
//!   graphs ([`CASES`] cases, failures kept in `rates.proptest-regressions`),
//!   with feedback, modulation across rates, events, suspends and resumes,
//!   and swaps that add, remove (directly or suspend-then-swap) and rewire,
//!   including cutting a cross-rate connection, which removes its converter
//!   (suspended first, or not); each at three engine configurations.
//! - `the_rate_generator_covers_its_dimensions`: what those cases reach.
//! - `one_graph_with_every_divisor_and_size_matches_in_every_configuration`:
//!   a fixed graph, every block size of a matrix, partitions and threads.
//! - `a_panic_in_a_one_tick_call_discards_later_parameter_events`
//!   (auditor-1-002, auditor-1-003): a divisor-64 call of one tick that
//!   panics, with a parameter set after the tick in the same call.

mod common;

use std::collections::BTreeSet;

use common::rates::{DIVISORS, SIZES, divided_type, registry, vector_type};
use common::{Setup, compare, run_engine, shuffled};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use signal_core::{
    Curve, Direction, EventPayload, Feedthrough, InstanceAddress, ModuleDescriptor, PortAddress,
    PortShape, Registry, SplitMix64,
};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_oracle::{Connection, Edit, ExternalEvent, Graph, Instance, Record, Script, Swap};

/// The generated cases (fixed, whatever `PROPTEST_CASES` says).
const CASES: u32 = 48;

const REGRESSIONS: FileFailurePersistence = FileFailurePersistence::Direct(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/rates.proptest-regressions"
));

struct Rng(SplitMix64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        (self.0.next_u64() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
    fn between(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * ((self.0.next_u64() >> 11) as f64 / (1u64 << 53) as f64)
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.below(items.len()))
        }
    }
}

#[derive(Clone, Debug)]
struct Node {
    name: String,
    type_name: String,
    parameters: Vec<(String, f64)>,
    /// A direct node reads lower ranks only (or state-output nodes).
    rank: usize,
}

#[derive(Clone, Debug, Default)]
struct Spec {
    nodes: Vec<Node>,
    /// (from, to, scale)
    connections: Vec<(String, String, Option<f64>)>,
}

/// A port: its node, name, shape, divisor and whether it is a parameter.
#[derive(Clone, Debug)]
struct Port {
    node: String,
    name: &'static str,
    shape: PortShape,
    rate: u32,
    parameter: bool,
}

fn descriptor(registry: &Registry, type_name: &str) -> ModuleDescriptor {
    registry.get(type_name).unwrap().descriptor
}

/// A type's divisor (its outputs' and parameters').
fn rate(registry: &Registry, type_name: &str) -> u32 {
    descriptor(registry, type_name).rate().unwrap().get()
}

/// Whether a source port may feed a target port: equal shapes (a signal
/// into a parameter), divisors equal or one a multiple of the other; events
/// only at equal divisors.
fn joins(source: &Port, target: &Port) -> bool {
    let (s, t) = (source.rate, target.rate);
    let commensurate = s % t == 0 || t % s == 0;
    match (source.shape, target.parameter) {
        (PortShape::Events, _) => s == t && (target.parameter || target.shape == PortShape::Events),
        (PortShape::Signal, true) => commensurate,
        (_, true) => false,
        (shape, false) => shape == target.shape && commensurate,
    }
}

struct Generator<'r> {
    registry: &'r Registry,
    rng: Rng,
    /// Standard types at every divisor (no sink, latency or event ports
    /// beyond divisor 1), and the vector fixtures.
    types: Vec<String>,
    next_rank: usize,
}

impl Generator<'_> {
    fn state_output(&self, type_name: &str) -> bool {
        descriptor(self.registry, type_name).feedthrough == Feedthrough::StateOutput
    }

    /// A type: a vector fixture, a unit delay at some divisor (feedback),
    /// or anything else.
    fn pick_type(&mut self) -> String {
        let d = DIVISORS[self.rng.below(DIVISORS.len())];
        match self.rng.below(10) {
            0..=2 => {
                let kind = ["spread", "gain", "mix", "delay"][self.rng.below(4)];
                vector_type(kind, SIZES[self.rng.below(SIZES.len())], d)
            }
            3 => divided_type("std.unit_delay", d),
            _ => self.types[self.rng.below(self.types.len())].clone(),
        }
    }

    fn node(&mut self, name: String, type_name: String) -> Node {
        let d = descriptor(self.registry, &type_name);
        let mut parameters = Vec::new();
        for p in d.parameters {
            if self.rng.chance(50) {
                let (lo, hi) = (p.range.min.max(-1e3), p.range.max.min(1e3));
                let value = if lo > hi {
                    p.range.min
                } else {
                    self.rng.between(lo, hi)
                };
                parameters.push((p.name.to_owned(), value));
            }
        }
        self.next_rank += 1;
        Node {
            name,
            type_name,
            parameters,
            rank: self.next_rank,
        }
    }

    fn ports(&self, node: &Node, direction: Direction) -> Vec<Port> {
        let d = descriptor(self.registry, &node.type_name);
        let mut ports: Vec<Port> = d
            .ports
            .iter()
            .filter(|p| p.direction == direction)
            .map(|p| Port {
                node: node.name.clone(),
                name: p.name,
                shape: p.shape,
                rate: p.rate.get(),
                parameter: false,
            })
            .collect();
        if direction == Direction::In {
            ports.extend(d.parameters.iter().map(|p| Port {
                node: node.name.clone(),
                name: p.name,
                shape: PortShape::Signal,
                rate: p.rate.get(),
                parameter: true,
            }));
        }
        ports
    }

    fn port(&self, spec: &Spec, dotted: &str, direction: Direction) -> Option<Port> {
        let (node, port) = dotted.split_once('.')?;
        let node = spec.nodes.iter().find(|n| n.name == node)?;
        self.ports(node, direction)
            .into_iter()
            .find(|p| p.name == port)
    }

    fn sources(&self, spec: &Spec, target: &Node, port: &Port) -> Vec<String> {
        let mut found = Vec::new();
        for node in &spec.nodes {
            if node.rank >= target.rank && !self.state_output(&node.type_name) {
                continue;
            }
            for out in self.ports(node, Direction::Out) {
                if joins(&out, port) {
                    found.push(format!("{}.{}", out.node, out.name));
                }
            }
        }
        found
    }

    fn wire(&mut self, spec: &mut Spec, node: &Node) {
        for port in self.ports(node, Direction::In) {
            let count = match self.rng.below(10) {
                0..=2 => 0,
                3..=7 => 1,
                _ => 2,
            };
            let candidates = self.sources(spec, node, &port);
            for _ in 0..count {
                let Some(from) = self.rng.pick(&candidates).cloned() else {
                    break;
                };
                self.connect(spec, from, format!("{}.{}", node.name, port.name));
            }
        }
    }

    fn connect(&mut self, spec: &mut Spec, from: String, to: String) {
        if spec
            .connections
            .iter()
            .any(|(f, t, _)| *f == from && *t == to)
        {
            return;
        }
        let signal = self
            .port(spec, &from, Direction::Out)
            .is_some_and(|p| p.shape != PortShape::Events);
        let scale = match (signal, self.rng.below(4)) {
            (false, _) | (_, 0) => None,
            (_, 1) => Some(0.5),
            (_, 2) => Some(-1.0),
            _ => Some(self.rng.between(-2.0, 2.0)),
        };
        spec.connections.push((from, to, scale));
    }

    /// Every signal (not vector) output, except the sink's.
    fn signal_outputs(&self, spec: &Spec) -> Vec<String> {
        spec.nodes
            .iter()
            .flat_map(|n| self.ports(n, Direction::Out))
            .filter(|p| p.shape == PortShape::Signal)
            .map(|p| format!("{}.{}", p.node, p.name))
            .collect()
    }

    /// The probe's inputs (divisor 1) from signal outputs of any divisor.
    fn wire_sink(&mut self, spec: &mut Spec) {
        spec.connections
            .retain(|(_, to, _)| !to.starts_with("out."));
        let outputs = self.signal_outputs(spec);
        let sink = spec.nodes.iter().find(|n| n.name == "out").unwrap().clone();
        for port in self.ports(&sink, Direction::In) {
            if self.rng.chance(90)
                && let Some(from) = self.rng.pick(&outputs).cloned()
            {
                self.connect(spec, from, format!("out.{}", port.name));
            }
        }
    }

    /// The divisors of `spec`'s cross-rate signal connections: (from, to
    /// divisor), each once.
    fn converted(&self, spec: &Spec) -> BTreeSet<(String, u32)> {
        spec.connections
            .iter()
            .filter_map(|(from, to, _)| {
                let s = self.port(spec, from, Direction::Out)?;
                let t = self.port(spec, to, Direction::In)?;
                (s.shape != PortShape::Events && s.rate != t.rate).then(|| (from.clone(), t.rate))
            })
            .collect()
    }
}

/// The converter of `from` (`node.port`) at divisor `t` (§16).
fn converter(from: &str, t: u32) -> String {
    let (node, port) = from.split_once('.').unwrap();
    format!("{node}/{port}_d{t}")
}

fn address(name: &str) -> InstanceAddress {
    format!("/{name}").parse().unwrap()
}

fn port_address(dotted: &str) -> PortAddress {
    let (node, port) = dotted.split_once('.').unwrap();
    PortAddress::new(&address(node), port).unwrap()
}

fn composite(spec: &Spec) -> CompositeDef {
    CompositeDef {
        name: "project".to_owned(),
        instances: spec
            .nodes
            .iter()
            .map(|n| InstanceDef {
                name: n.name.clone(),
                module: n.type_name.clone(),
                parameters: n.parameters.iter().cloned().collect(),
            })
            .collect(),
        connections: spec
            .connections
            .iter()
            .map(|(from, to, scale)| ConnectionDef {
                from: from.clone(),
                to: to.clone(),
                scale: *scale,
            })
            .collect(),
        exports: Exports::default(),
    }
}

/// The oracle's graph: the spec itself (no converters), shuffled.
fn graph(spec: &Spec, version: u64, seed: u64) -> Graph {
    let instances = spec
        .nodes
        .iter()
        .map(|n| Instance {
            address: address(&n.name),
            type_name: n.type_name.clone(),
            parameters: n.parameters.clone(),
        })
        .collect();
    let connections = spec
        .connections
        .iter()
        .map(|(from, to, scale)| Connection {
            from: port_address(from),
            to: port_address(to),
            scale: scale.unwrap_or(1.0),
        })
        .collect();
    Graph {
        version,
        instances: shuffled(instances, seed),
        connections: shuffled(connections, !seed),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Draw {
    seed: u64,
    instances: usize,
    frames: u64,
    rate: u32,
    swaps: usize,
}

fn draw() -> impl Strategy<Value = Draw> {
    (
        any::<u64>(),
        3usize..=9,
        256u64..=2048,
        prop_oneof![Just(1_000u32), Just(4_000), Just(48_000)],
        0usize..=2,
    )
        .prop_map(|(seed, instances, frames, rate, swaps)| Draw {
            seed,
            instances,
            frames,
            rate,
            swaps,
        })
}

/// What a case is about, for the coverage test.
#[derive(Default)]
struct Notes {
    /// Converters suspended before the swap that removes them.
    suspended_converters: usize,
    /// Swaps that cut a cross-rate connection.
    cuts: usize,
    /// Removed nodes that fed a converter at a divisor above 1.
    removed_sources: usize,
}

struct Case {
    plan: ExecutionPlan,
    swaps: Vec<ExecutionPlan>,
    graph: Graph,
    script: Script,
    notes: Notes,
}

fn build(registry: &Registry, draw: &Draw) -> Case {
    let mut types: Vec<String> = registry
        .iter()
        .map(|t| t.descriptor)
        .filter(|d| {
            d.sink.is_none()
                && d.latency.is_none()
                && (d.type_name.starts_with("std.") || d.type_name.starts_with('d'))
        })
        .map(|d| d.type_name.to_owned())
        .collect();
    types.sort();
    let mut g = Generator {
        registry,
        rng: Rng(SplitMix64::new(draw.seed)),
        types,
        next_rank: 0,
    };
    let fade = ((u64::from(draw.rate) * 5 + 500) / 1000).max(1);
    let mut notes = Notes::default();

    let mut spec = Spec::default();
    for k in 0..draw.instances {
        let type_name = g.pick_type();
        let node = g.node(format!("n{k}"), type_name);
        spec.nodes.push(node);
    }
    for k in 0..spec.nodes.len() {
        let node = spec.nodes[k].clone();
        g.wire(&mut spec, &node);
    }
    let mut sink = g.node("out".to_owned(), "oracle.probe".to_owned());
    sink.rank = usize::MAX;
    spec.nodes.push(sink);
    g.wire_sink(&mut spec);

    let mut specs = vec![spec];
    let mut swap_frames = Vec::new();
    let mut changed: BTreeSet<String> = BTreeSet::new();
    let mut edits = Vec::new();
    let mut at = 1 + g.rng.below(draw.frames as usize / 2) as u64;
    for k in 0..draw.swaps {
        let mut next = specs.last().unwrap().clone();
        let movable: Vec<String> = next
            .nodes
            .iter()
            .filter(|n| n.name != "out")
            .map(|n| n.name.clone())
            .collect();
        let mut frame = at;
        match g.rng.below(4) {
            0 => {
                let type_name = g.pick_type();
                let node = g.node(format!("a{k}"), type_name);
                next.nodes.push(node.clone());
                g.wire(&mut next, &node);
                g.wire_sink(&mut next);
            }
            1 if movable.len() > 1 => {
                let name = g.rng.pick(&movable).unwrap().clone();
                let first = specs[0].nodes.iter().any(|n| n.name == name);
                if first && !changed.contains(&name) && g.rng.chance(50) {
                    edits.push(Edit {
                        frame: at,
                        instance: address(&name),
                        resume: false,
                    });
                    frame = at + fade;
                }
                let prefix = format!("{name}.");
                if g.converted(&next)
                    .iter()
                    .any(|(from, t)| from.starts_with(&prefix) && *t > 1)
                {
                    notes.removed_sources += 1;
                }
                changed.insert(name.clone());
                next.nodes.retain(|n| n.name != name);
                next.connections
                    .retain(|(f, t, _)| !f.starts_with(&prefix) && !t.starts_with(&prefix));
                g.wire_sink(&mut next);
            }
            // Cut a cross-rate connection's converter: every connection of
            // its source port to targets at its divisor.
            2 => {
                let cut: Vec<(String, u32)> = g.converted(&next).into_iter().collect();
                if let Some((from, t)) = g.rng.pick(&cut).cloned() {
                    notes.cuts += 1;
                    let name = converter(&from, t);
                    let existed = g.converted(&specs[0]).contains(&(from.clone(), t));
                    if existed && !changed.contains(&name) && g.rng.chance(50) {
                        edits.push(Edit {
                            frame: at,
                            instance: address(&name),
                            resume: false,
                        });
                        frame = at + fade;
                        notes.suspended_converters += 1;
                    }
                    changed.insert(name);
                    let joined: Vec<String> = next
                        .connections
                        .iter()
                        .filter(|(f, to, _)| {
                            *f == from
                                && g.port(&next, to, Direction::In)
                                    .is_some_and(|p| p.rate == t)
                        })
                        .map(|(_, to, _)| to.clone())
                        .collect();
                    next.connections
                        .retain(|(f, to, _)| !(*f == from && joined.contains(to)));
                }
            }
            _ => {
                if let Some(name) = g.rng.pick(&movable).cloned() {
                    let to = format!("{name}.");
                    next.connections.retain(|(_, t, _)| !t.starts_with(&to));
                    let node = next.nodes.iter().find(|n| n.name == name).unwrap().clone();
                    g.wire(&mut next, &node);
                }
                g.wire_sink(&mut next);
            }
        }
        if frame >= draw.frames {
            break;
        }
        swap_frames.push(frame);
        specs.push(next);
        at = frame + 1 + g.rng.below(draw.frames as usize / 4 + 1) as u64;
    }

    // At least three ticks of the slowest divisor.
    let slowest = specs
        .iter()
        .flat_map(|s| &s.nodes)
        .map(|n| rate(registry, &n.type_name))
        .max()
        .unwrap_or(1);
    let frames = draw.frames.max(3 * u64::from(slowest));

    let stable: Vec<String> = specs[0]
        .nodes
        .iter()
        .map(|n| n.name.clone())
        .filter(|n| !changed.contains(n))
        .collect();
    for _ in 0..g.rng.below(3) {
        let Some(name) = g.rng.pick(&stable).cloned() else {
            break;
        };
        if edits.iter().any(|e| e.instance == address(&name)) {
            continue;
        }
        let s = g.rng.below(frames as usize) as u64;
        edits.push(Edit {
            frame: s,
            instance: address(&name),
            resume: false,
        });
        if g.rng.chance(70) {
            let r = s + 1 + g.rng.below(2 * fade as usize + 80) as u64;
            if r < frames {
                edits.push(Edit {
                    frame: r,
                    instance: address(&name),
                    resume: true,
                });
            }
        }
    }
    edits.sort_by_key(|e| e.frame);

    let mut targets = Vec::new();
    for node in &specs[0].nodes {
        for port in g.ports(node, Direction::In) {
            if port.parameter || port.shape == PortShape::Events {
                targets.push(format!("{}.{}", port.node, port.name));
            }
        }
    }
    let mut events = Vec::new();
    for sequence in 1..=g.rng.below(13) as u64 {
        let Some(target) = g.rng.pick(&targets).cloned() else {
            break;
        };
        let payload = match g.rng.below(5) {
            0 | 1 => EventPayload::Set(g.rng.between(-2.0, 2.0)),
            2 | 3 => EventPayload::Ramp {
                to: g.rng.between(-2.0, 2.0),
                frames: g.rng.below(200) as u64,
                curve: Curve::Linear,
            },
            _ => EventPayload::Trigger,
        };
        events.push(ExternalEvent {
            frame: g.rng.below(frames as usize) as u64,
            sequence,
            target: port_address(&target),
            payload,
        });
    }

    let plans: Vec<ExecutionPlan> = specs
        .iter()
        .enumerate()
        .map(|(v, spec)| {
            build_plan(&composite(spec), &[], registry, v as u64 + 1)
                .unwrap_or_else(|e| panic!("an invalid generated graph: {e:?}\n{spec:?}"))
        })
        .collect();
    let script = Script {
        sample_rate: draw.rate,
        seed: draw.seed ^ 0x5eed,
        frames,
        events,
        edits,
        swaps: swap_frames
            .iter()
            .zip(&specs[1..])
            .enumerate()
            .map(|(k, (&frame, spec))| Swap {
                frame,
                graph: graph(spec, k as u64 + 2, draw.seed.wrapping_add(k as u64)),
            })
            .collect(),
    };
    let mut plans = plans.into_iter();
    Case {
        plan: plans.next().unwrap(),
        swaps: plans.collect(),
        graph: graph(&specs[0], 1, draw.seed),
        script,
        notes,
    }
}

/// Three engine configurations: block sizes, threads (the first more than
/// one) and partitions.
fn setups(seed: u64, frames: u64) -> Vec<Setup> {
    let mut rng = Rng(SplitMix64::new(seed ^ 0xb10c));
    (0..3)
        .map(|k| {
            let block = match rng.below(3) {
                0 => *rng.pick(&[1usize, 2, 3, 7, 64, 128, 256]).unwrap(),
                _ => 1 + rng.below(256),
            };
            let threads = if k == 0 {
                *rng.pick(&[2usize, 3, 4, 8]).unwrap()
            } else {
                *rng.pick(&[1usize, 2, 4, 8]).unwrap()
            };
            let splits = (0..rng.below(9))
                .map(|_| 1 + rng.below(frames as usize - 1) as u64)
                .collect();
            Setup {
                block,
                threads,
                splits,
            }
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: CASES,
        failure_persistence: Some(Box::new(REGRESSIONS)),
        ..ProptestConfig::default()
    })]

    #[test]
    fn the_block_engine_matches_the_oracle_with_rates_and_vectors(draw in draw()) {
        let registry = registry();
        let case = build(&registry, &draw);
        let oracle = signal_oracle::run(&registry, &case.graph, &case.script)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        for setup in setups(draw.seed, case.script.frames) {
            let engine = run_engine(&registry, &case.plan, &case.swaps, &case.script, &setup, &oracle);
            if let Err(difference) = compare(&oracle, &engine) {
                return Err(TestCaseError::fail(format!("{setup:?}: {difference}")));
            }
        }
    }
}

/// The generator reaches what AC1 asks for over the cases' seeds: every
/// divisor and vector size, holds and decimates (shared, into parameters),
/// both feedthrough classes above divisor 1, feedback through a divided
/// state-output instance, converters added by a swap and removed (suspended
/// first, and directly), a removed source of a converter above divisor 1,
/// and suspends of divided instances.
#[test]
fn the_rate_generator_covers_its_dimensions() {
    let registry = registry();
    let mut divisors = BTreeSet::new();
    let mut sizes = BTreeSet::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let (mut suspended_converters, mut cuts, mut removed_sources) = (0, 0, 0);
    for seed in 0..u64::from(CASES) * 4 {
        let draw = Draw {
            seed,
            instances: 3 + (seed % 7) as usize,
            frames: 512,
            rate: 1_000,
            swaps: (seed % 3) as usize,
        };
        let case = build(&registry, &draw);
        suspended_converters += case.notes.suspended_converters;
        cuts += case.notes.cuts;
        removed_sources += case.notes.removed_sources;
        let plans: Vec<&ExecutionPlan> = std::iter::once(&case.plan).chain(&case.swaps).collect();
        for (v, plan) in plans.iter().enumerate() {
            let rate_of = |a: &InstanceAddress| {
                plan.instances
                    .iter()
                    .find(|i| i.address == *a)
                    .map(|i| i.descriptor.rate().unwrap().get())
            };
            let converters: BTreeSet<&InstanceAddress> = plan
                .instances
                .iter()
                .filter(|i| i.type_name.starts_with("core."))
                .map(|i| &i.address)
                .collect();
            for i in &plan.instances {
                let d = i.descriptor.rate().unwrap().get();
                divisors.insert(d);
                for p in i.descriptor.ports {
                    if let PortShape::Vector(n) = p.shape {
                        sizes.insert(n);
                    }
                }
                if i.type_name.starts_with("core.hold") {
                    seen.insert("hold");
                }
                if i.type_name.starts_with("core.decimate") {
                    seen.insert("decimate");
                }
                if d > 1 && !i.type_name.starts_with("core.") {
                    match i.descriptor.feedthrough {
                        Feedthrough::Direct => seen.insert("divided direct"),
                        Feedthrough::StateOutput => seen.insert("divided state-output"),
                    };
                }
                if v > 0
                    && converters.contains(&i.address)
                    && !plans[v - 1]
                        .instances
                        .iter()
                        .any(|p| p.address == i.address)
                {
                    seen.insert("converter added by a swap");
                }
            }
            for (to, sources) in &plan.sources {
                let from_converter = sources
                    .iter()
                    .filter(|s| converters.contains(&s.from.instance()))
                    .count();
                if from_converter > 0
                    && plan.instances.iter().any(|i| {
                        i.address == to.instance()
                            && i.descriptor.parameters.iter().any(|p| p.name == to.port())
                    })
                {
                    seen.insert("conversion into a parameter");
                }
                for s in sources {
                    let from = s.from.instance();
                    let position = |a: &InstanceAddress| plan.schedule.iter().position(|p| p == a);
                    let divided_state = plan.instances.iter().any(|i| {
                        i.address == from
                            && i.descriptor.feedthrough == Feedthrough::StateOutput
                            && rate_of(&from).unwrap() > 1
                    });
                    if divided_state && position(&from) >= position(&to.instance()) {
                        seen.insert("feedback through a divided state-output");
                    }
                }
            }
            for c in &converters {
                let targets = plan
                    .sources
                    .values()
                    .flatten()
                    .filter(|s| s.from.instance() == **c)
                    .count();
                if targets > 1 {
                    seen.insert("shared converter");
                }
            }
        }
        for e in &case.script.edits {
            let divided = case.plan.instances.iter().any(|i| {
                i.address == e.instance
                    && !i.type_name.starts_with("core.")
                    && i.descriptor.rate().unwrap().get() > 1
            });
            if divided && !e.resume {
                seen.insert("suspend of a divided instance");
            }
        }
    }
    assert_eq!(divisors, BTreeSet::from(DIVISORS), "divisors");
    assert_eq!(sizes, BTreeSet::from(SIZES), "vector sizes");
    for what in [
        "hold",
        "decimate",
        "divided direct",
        "divided state-output",
        "converter added by a swap",
        "conversion into a parameter",
        "feedback through a divided state-output",
        "shared converter",
        "suspend of a divided instance",
    ] {
        assert!(seen.contains(what), "the generator made no {what}");
    }
    assert!(
        suspended_converters > 0,
        "no converter suspended before its removal"
    );
    assert!(cuts > suspended_converters, "no converter removed directly");
    assert!(
        removed_sources > 0,
        "no removed source of a converter above divisor 1"
    );
}

/// The node `name` of type `type_name` with `parameters`.
fn node(name: &str, type_name: &str, parameters: &[(&str, f64)]) -> Node {
    Node {
        name: name.to_owned(),
        type_name: type_name.to_owned(),
        parameters: parameters.iter().map(|&(k, v)| (k.to_owned(), v)).collect(),
        rank: 0,
    }
}

fn wire(from: &str, to: &str, scale: Option<f64>) -> (String, String, Option<f64>) {
    (from.to_owned(), to.to_owned(), scale)
}

/// The engine's plan and the oracle's graph of `spec`.
fn both(registry: &Registry, spec: &Spec) -> (ExecutionPlan, Graph) {
    let plan = build_plan(&composite(spec), &[], registry, 1).unwrap();
    (plan, graph(spec, 1, 7))
}

/// Runs `script` at every setup in `setups`; each must equal the oracle.
fn matches(registry: &Registry, spec: &Spec, script: &Script, setups: &[Setup]) -> Vec<Record> {
    let (plan, graph) = both(registry, spec);
    let oracle = signal_oracle::run(registry, &graph, script).unwrap();
    for setup in setups {
        let engine = run_engine(registry, &plan, &[], script, setup, &oracle);
        if let Err(difference) = compare(&oracle, &engine) {
            panic!("{setup:?}: {difference}");
        }
    }
    oracle.log
}

/// One graph with every divisor of AC1 and every vector size: oscillators at
/// 1, 2, 3, 64 and 1600; spreads to vectors of 1, 4 and 256 elements, a gain
/// and a delay in a feedback loop at 64, mixes back to signals; holds into
/// the probe and into parameters (shared), decimates into the vectors; events
/// at frames that are no tick; a suspended and resumed divided instance.
/// Every block size, two partitions and one and four threads.
#[test]
fn one_graph_with_every_divisor_and_size_matches_in_every_configuration() {
    let registry = registry();
    let spec = Spec {
        nodes: vec![
            node("osc", "std.sine_osc", &[("freq", 440.0)]),
            node("s2", &divided_type("std.sine_osc", 2), &[("freq", 300.0)]),
            node("s3", &divided_type("std.sine_osc", 3), &[("freq", 200.0)]),
            node("l64", &divided_type("std.sine_osc", 64), &[("freq", 3.0)]),
            node(
                "l1600",
                &divided_type("std.sine_osc", 1600),
                &[("freq", 1.0)],
            ),
            node("v1", &vector_type("spread", 1, 2), &[]),
            node("v4", &vector_type("spread", 4, 64), &[]),
            node("v256", &vector_type("spread", 256, 1600), &[]),
            node("g4", &vector_type("gain", 4, 64), &[("gain", 0.5)]),
            node("dl4", &vector_type("delay", 4, 64), &[]),
            node("m1", &vector_type("mix", 1, 2), &[]),
            node("m4", &vector_type("mix", 4, 64), &[]),
            node("m256", &vector_type("mix", 256, 1600), &[]),
            node("ud3", &divided_type("std.unit_delay", 3), &[]),
            node("out", "oracle.probe", &[]),
        ],
        connections: vec![
            wire("osc.out", "v1.in", None),
            wire("s2.out", "v4.in", None),
            wire("l64.out", "v256.in", None),
            wire("v4.out", "g4.in", None),
            wire("dl4.out", "g4.in", Some(0.25)),
            wire("l1600.out", "g4.gain", Some(0.5)),
            wire("g4.out", "dl4.in", None),
            wire("v1.out", "m1.in", None),
            wire("dl4.out", "m4.in", None),
            wire("v256.out", "m256.in", None),
            wire("s3.out", "ud3.in", None),
            wire("l64.out", "osc.freq", Some(100.0)),
            wire("osc.out", "out.in0", None),
            wire("s2.out", "out.in1", None),
            wire("ud3.out", "out.in2", None),
            wire("m1.out", "out.in3", None),
            wire("m4.out", "out.in4", None),
            wire("m256.out", "out.in5", None),
            wire("l64.out", "out.in6", Some(0.5)),
            wire("s2.out", "out.in7", Some(-1.0)),
        ],
    };
    let set = |frame, sequence, target: &str, payload| ExternalEvent {
        frame,
        sequence,
        target: port_address(target),
        payload,
    };
    let script = Script {
        sample_rate: 48_000,
        seed: 11,
        frames: 4_800,
        events: vec![
            set(101, 1, "s3.freq", EventPayload::Set(500.0)),
            set(
                1_601,
                2,
                "g4.gain",
                EventPayload::Ramp {
                    to: -1.0,
                    frames: 700,
                    curve: Curve::Linear,
                },
            ),
            set(3_203, 3, "v256.gain", EventPayload::Set(0.25)),
        ],
        edits: vec![
            Edit {
                frame: 1_000,
                instance: address("l64"),
                resume: false,
            },
            Edit {
                frame: 1_130,
                instance: address("l64"),
                resume: true,
            },
        ],
        swaps: Vec::new(),
    };
    let mut setups = Vec::new();
    for block in [1, 3, 64, 256] {
        for threads in [1, 4] {
            for splits in [Vec::new(), vec![63, 64, 65, 1_599, 1_601, 3_201]] {
                setups.push(Setup {
                    block,
                    threads,
                    splits,
                });
            }
        }
    }
    let (plan, _) = both(&registry, &spec);
    let converters: Vec<&str> = plan
        .instances
        .iter()
        .filter(|i| i.type_name.starts_with("core."))
        .map(|i| i.type_name)
        .collect();
    for d in DIVISORS {
        assert!(
            plan.instances
                .iter()
                .any(|i| i.descriptor.rate().unwrap().get() == d),
            "divisor {d}"
        );
    }
    assert!(converters.iter().any(|t| t.starts_with("core.hold")));
    assert!(converters.iter().any(|t| t.starts_with("core.decimate")));
    matches(&registry, &spec, &script, &setups);
}

/// auditor-1-002 and -003: `fail.d64` panics in a call whose tick sees its
/// `arm` at 1 (set at frame 10), at its tick 64; a set at frame 65, after
/// that tick in the same call, must be discarded as `muted`, as one frame
/// at a time does. Only partitions where tick 64 is alone in its call are
/// exact (`docs/semantics.md` §14); every one here is: blocks of 1, 2, 3, 7
/// and 64, and of 128 and 256 split at 1 and 66 (the call `[1, 66)`).
#[test]
fn a_panic_in_a_one_tick_call_discards_later_parameter_events() {
    let registry = registry();
    let spec = Spec {
        nodes: vec![
            node("fail", "fail.d64", &[]),
            node("out", "oracle.probe", &[]),
        ],
        connections: vec![wire("fail.out", "out.in0", None)],
    };
    let set = |frame, sequence, value| ExternalEvent {
        frame,
        sequence,
        target: port_address("fail.arm"),
        payload: EventPayload::Set(value),
    };
    let script = Script {
        sample_rate: 48_000,
        seed: 3,
        frames: 300,
        events: vec![set(10, 1, 1.0), set(65, 2, 0.5)],
        edits: Vec::new(),
        swaps: Vec::new(),
    };
    let mut setups = Vec::new();
    for threads in [1, 4] {
        for block in [1, 2, 3, 7, 64] {
            setups.push(Setup {
                block,
                threads,
                splits: Vec::new(),
            });
        }
        for block in [128, 256] {
            setups.push(Setup {
                block,
                threads,
                splits: vec![1, 66],
            });
        }
    }
    let log = matches(&registry, &spec, &script, &setups);
    assert!(log.iter().any(|r| matches!(
        r,
        Record::Fault { frame: 64, address, .. } if *address == self::address("fail")
    )));
    assert!(log.iter().any(|r| matches!(
        r,
        Record::Discard {
            frame: 65,
            reason: signal_oracle::DiscardReason::Muted,
            ..
        }
    )));
    assert!(
        !log.iter()
            .any(|r| matches!(r, Record::Event { frame: 65, .. }))
    );
}
