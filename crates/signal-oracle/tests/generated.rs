//! AC-GEN, AC3 (M2.7-6): random valid graphs of the standard primitives,
//! with events, feedback through `std.unit_delay` and `std.delay`,
//! parameters with modulation and scripted edits (set, ramp and trigger
//! events, suspends and resumes, plan swaps that add, remove, rewire and
//! retype), run by the oracle and by the block engine at three
//! configurations each (a block size, a thread count, a partition), which
//! must match bit for bit: audio, log, final parameters and instance states.
//!
//! Each case's graph is built twice from one random spec: as the engine's
//! `CompositeDef` (through `signal_graph::build_plan`) and as the oracle's
//! `Graph`, its connections shuffled. A configuration with more than one
//! thread runs every segment on the workers (`Schedule::FORCED`) and must
//! show it: at least two tasks, a parallel segment and a task a worker ran.
//!
//! 256 cases (fixed, whatever `PROPTEST_CASES` says); failures are recorded
//! in `generated.proptest-regressions` beside this file and replayed first.
//! The file is named directly ([`REGRESSIONS`]): `SourceParallel` looks for
//! a `lib.rs` or `main.rs` above `tests/`, finds none, and only guesses.

mod common;

use std::cell::RefCell;
use std::collections::BTreeSet;

use common::{Setup, compare, registry, run_engine, shuffled};
use proptest::prelude::*;
use proptest::test_runner::{FailurePersistence, FileFailurePersistence, TestError, TestRunner};
use signal_core::{
    Curve, Direction, EventPayload, Feedthrough, InstanceAddress, PortAddress, PortShape, Range,
    Registry, SplitMix64,
};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_oracle::{Connection, Edit, ExternalEvent, Graph, Instance, PROBE_INPUTS, Script, Swap};

/// The cases run.
const CASES: u32 = 256;

/// The regression file, whose seeds are replayed before new cases.
const REGRESSIONS: FileFailurePersistence = FileFailurePersistence::Direct(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/generated.proptest-regressions"
));

/// A random stream for one case.
struct Rng(SplitMix64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        (self.0.next_u64() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
    fn unit(&mut self) -> f64 {
        (self.0.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn between(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.unit()
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.below(items.len()))
        }
    }
}

/// One instance of a spec: its name, type, initial parameters and rank
/// (a direct instance reads only lower ranks, or state-output instances).
#[derive(Clone, Debug)]
struct Node {
    name: String,
    type_name: &'static str,
    parameters: Vec<(String, f64)>,
    rank: usize,
}

/// A graph spec: instances and connections (`inst.port` to `inst.port`).
#[derive(Clone, Debug, Default)]
struct Spec {
    nodes: Vec<Node>,
    /// (from, to, scale)
    connections: Vec<(String, String, Option<f64>)>,
}

/// A port of a node: (node, port name, shape, direction, is a parameter).
#[derive(Clone, Debug)]
struct Port {
    node: String,
    name: &'static str,
    shape: PortShape,
    parameter: bool,
}

struct Generator<'r> {
    registry: &'r Registry,
    rng: Rng,
    /// The types instances are drawn from.
    types: Vec<&'static str>,
    next_rank: usize,
}

impl Generator<'_> {
    fn descriptor(&self, type_name: &str) -> signal_core::ModuleDescriptor {
        self.registry.get(type_name).unwrap().descriptor
    }

    fn state_output(&self, type_name: &str) -> bool {
        self.descriptor(type_name).feedthrough == Feedthrough::StateOutput
    }

    /// A type, with `unit_delay` and `delay` (the feedback types) likelier.
    fn pick_type(&mut self) -> &'static str {
        match self.rng.below(10) {
            0 => "std.unit_delay",
            1 => "std.delay",
            _ => self.types[self.rng.below(self.types.len())],
        }
    }

    /// An initial parameter value in its range (validation refuses others;
    /// `set` events take bases outside it), within ±10⁴ where the range is
    /// wider.
    fn value(&mut self, range: Range) -> f64 {
        let lo = range.min.max(-1e4);
        let hi = range.max.min(1e4);
        if lo > hi {
            range.min
        } else {
            self.rng.between(lo, hi)
        }
    }

    fn node(&mut self, name: String, type_name: &'static str) -> Node {
        let descriptor = self.descriptor(type_name);
        let mut parameters = Vec::new();
        for p in descriptor.parameters {
            if self.rng.chance(60) {
                // ADR 0043: a discrete parameter takes one of its values.
                let value = match p.values {
                    Some(values) if !values.is_empty() => {
                        let at = self.rng.between(0.0, values.len() as f64) as usize;
                        values[at.min(values.len() - 1)].value
                    }
                    _ => self.value(p.range),
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

    /// Every port of `node` with `direction`.
    fn ports(&self, node: &Node, direction: Direction) -> Vec<Port> {
        let descriptor = self.descriptor(node.type_name);
        let mut ports: Vec<Port> = descriptor
            .ports
            .iter()
            .filter(|p| p.direction == direction)
            .map(|p| Port {
                node: node.name.clone(),
                name: p.name,
                shape: p.shape,
                parameter: false,
            })
            .collect();
        if direction == Direction::In {
            ports.extend(descriptor.parameters.iter().map(|p| Port {
                node: node.name.clone(),
                name: p.name,
                shape: PortShape::Signal,
                parameter: true,
            }));
        }
        ports
    }

    /// Sources `target` may read: outputs of lower-ranked instances and of
    /// any state-output instance (feedback), of a shape it takes.
    fn sources(&self, spec: &Spec, target: &Node, port: &Port) -> Vec<String> {
        let mut found = Vec::new();
        for node in &spec.nodes {
            if node.rank >= target.rank && !self.state_output(node.type_name) {
                continue;
            }
            for out in self.ports(node, Direction::Out) {
                let fits = match (port.parameter, port.shape, out.shape) {
                    (true, _, _) => true,
                    (false, a, b) => a == b,
                };
                if fits {
                    found.push(format!("{}.{}", out.node, out.name));
                }
            }
        }
        found
    }

    /// Connects each input and parameter of `node` to 0–2 sources, but
    /// never both inputs of a pair its type makes exclusive (ADR 0033: a
    /// sequencer's `tick` and `follow`): the second of a pair whose first
    /// is connected is left unconnected (its draws still made).
    fn wire(&mut self, spec: &mut Spec, node: &Node) {
        let pairs: Vec<(&str, &str)> = self.registry.exclusive_pairs(node.type_name).collect();
        for port in self.ports(node, Direction::In) {
            let count = match self.rng.below(10) {
                0..=3 => 0,
                4..=7 => 1,
                _ => 2,
            };
            let candidates = self.sources(spec, node, &port);
            let taken = pairs.iter().any(|&(a, b)| {
                (port.name == b || port.name == a) && {
                    let other = if port.name == b { a } else { b };
                    let to = format!("{}.{other}", node.name);
                    spec.connections.iter().any(|(_, t, _)| *t == to)
                }
            });
            for _ in 0..count {
                let Some(from) = self.rng.pick(&candidates).cloned() else {
                    break;
                };
                if !taken {
                    self.connect(spec, from, format!("{}.{}", node.name, port.name));
                }
            }
        }
    }

    /// Adds a connection unless it exists; a scale on signals only.
    fn connect(&mut self, spec: &mut Spec, from: String, to: String) {
        if spec
            .connections
            .iter()
            .any(|(f, t, _)| *f == from && *t == to)
        {
            return;
        }
        let signal = self.is_signal_output(spec, &from);
        let scale = if signal {
            match self.rng.below(5) {
                0 => None,
                1 => Some(-1.0),
                2 => Some(0.5),
                3 => Some(0.25),
                _ => Some(self.rng.between(-2.0, 2.0)),
            }
        } else {
            None
        };
        spec.connections.push((from, to, scale));
    }

    fn is_signal_output(&self, spec: &Spec, from: &str) -> bool {
        let (node, port) = from.split_once('.').unwrap();
        let node = spec.nodes.iter().find(|n| n.name == node).unwrap();
        self.descriptor(node.type_name)
            .ports
            .iter()
            .any(|p| p.name == port && p.shape == PortShape::Signal)
    }

    /// Every signal output of the spec, except the sink's.
    fn signal_outputs(&self, spec: &Spec) -> Vec<String> {
        spec.nodes
            .iter()
            .flat_map(|n| self.ports(n, Direction::Out))
            .filter(|p| p.shape == PortShape::Signal)
            .map(|p| format!("{}.{}", p.node, p.name))
            .collect()
    }

    /// Connects every input of the sink (`out`) to a random signal output.
    fn wire_sink(&mut self, spec: &mut Spec) {
        spec.connections
            .retain(|(_, to, _)| !to.starts_with("out."));
        let outputs = self.signal_outputs(spec);
        let sink = spec.nodes.iter().find(|n| n.name == "out").unwrap().clone();
        for port in self.ports(&sink, Direction::In) {
            if port.shape == PortShape::Signal
                && !port.parameter
                && self.rng.chance(90)
                && let Some(from) = self.rng.pick(&outputs).cloned()
            {
                self.connect(spec, from, format!("out.{}", port.name));
            }
        }
    }
}

/// One generated case: the engine's plans, the oracle's graph and the script.
struct Case {
    plan: ExecutionPlan,
    swaps: Vec<ExecutionPlan>,
    graph: Graph,
    script: Script,
}

fn composite(spec: &Spec) -> CompositeDef {
    CompositeDef {
        name: "project".to_owned(),
        instances: spec
            .nodes
            .iter()
            .map(|n| InstanceDef {
                name: n.name.clone(),
                module: n.type_name.to_owned(),
                parameters: n.parameters.iter().cloned().collect(),
                scope: None,
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

fn address(name: &str) -> InstanceAddress {
    format!("/{name}").parse().unwrap()
}

fn port_address(dotted: &str) -> PortAddress {
    let (node, port) = dotted.split_once('.').unwrap();
    PortAddress::new(&address(node), port).unwrap()
}

/// The oracle's graph of `spec`, built from the spec itself (not from a
/// plan), its instances and connections shuffled.
fn graph(spec: &Spec, version: u64, seed: u64) -> Graph {
    let instances = spec
        .nodes
        .iter()
        .map(|n| Instance {
            address: address(&n.name),
            type_name: n.type_name.to_owned(),
            parameters: n.parameters.clone(),
            stream: None,
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

/// The raw parameters a case is drawn from; proptest shrinks them.
#[derive(Clone, Debug, PartialEq)]
struct Draw {
    seed: u64,
    instances: usize,
    frames: u64,
    rate: u32,
    probe: bool,
    swaps: usize,
}

fn draw() -> impl Strategy<Value = Draw> {
    (
        any::<u64>(),
        2usize..=12,
        256u64..=2048,
        prop_oneof![Just(1_000u32), Just(4_000), Just(48_000)],
        prop::bool::weighted(0.75),
        0usize..=2,
    )
        .prop_map(|(seed, instances, frames, rate, probe, swaps)| Draw {
            seed,
            instances,
            frames,
            rate,
            probe,
            swaps,
        })
}

/// The standard types the generator draws: those at divisor 1 without
/// vector ports. The M2.8-7 types (`std.count`, the visual-rate physics and
/// `std.mesh2d_audio`) carry vectors, which the generator's wiring and its
/// probe (scalar signals) do not reach; the contract suite and the engine
/// tests cover them. From M2.10-1 the pool includes the core types
/// (`std.div` to `std.not`), so a seed drawn before then draws a different
/// graph (`docs/testing-and-evaluation.md`).
fn scalar(d: &signal_core::ModuleDescriptor) -> bool {
    d.rate() == Some(signal_core::RateDivisor::ONE)
        && d.ports
            .iter()
            .all(|p| !matches!(p.shape, signal_core::PortShape::Vector(_)))
}

fn build(registry: &Registry, draw: &Draw) -> Case {
    let types = registry
        .iter()
        .map(|t| t.descriptor)
        .filter(|d| d.sink.is_none() && d.latency.is_none() && d.type_name.starts_with("std."))
        .filter(scalar)
        .map(|d| d.type_name)
        .collect();
    let mut g = Generator {
        registry,
        rng: Rng(SplitMix64::new(draw.seed)),
        types,
        next_rank: 0,
    };
    let fade = ((u64::from(draw.rate) * 5 + 500) / 1000).max(1);

    // The initial graph.
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
    let sink_type = if draw.probe {
        "oracle.probe"
    } else {
        "std.audio_out"
    };
    let mut sink = g.node("out".to_owned(), sink_type);
    sink.rank = usize::MAX;
    spec.nodes.push(sink.clone());
    g.wire_sink(&mut spec);
    if !draw.probe && g.rng.chance(50) {
        // The sink's gain, modulated.
        if let Some(from) = g.rng.pick(&g.signal_outputs(&spec)).cloned() {
            g.connect(&mut spec, from, "out.gain".to_owned());
        }
    }

    let mut specs = vec![spec.clone()];
    let mut swap_frames = Vec::new();
    // Instances a swap removes or retypes: edits avoid them.
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
            // Add an instance, fed by the graph, perhaps heard.
            0 => {
                let type_name = g.pick_type();
                let node = g.node(format!("a{k}"), type_name);
                next.nodes.push(node.clone());
                g.wire(&mut next, &node);
                g.wire_sink(&mut next);
            }
            // Remove one, by suspend-then-swap or directly.
            1 if movable.len() > 1 => {
                let name = g.rng.pick(&movable).unwrap().clone();
                // Edits are scheduled before the run, so only for instances of
                // the first plan (`Engine::suspend`).
                let first = specs[0].nodes.iter().any(|n| n.name == name);
                if first && !changed.contains(&name) && g.rng.chance(50) {
                    edits.push(Edit {
                        frame: at,
                        instance: address(&name),
                        resume: false,
                    });
                    frame = at + fade;
                }
                changed.insert(name.clone());
                next.nodes.retain(|n| n.name != name);
                let prefix = format!("{name}.");
                next.connections
                    .retain(|(f, t, _)| !f.starts_with(&prefix) && !t.starts_with(&prefix));
                g.wire_sink(&mut next);
            }
            // Retype one: a removal and an addition at its address.
            2 if !movable.is_empty() => {
                let name = g.rng.pick(&movable).unwrap().clone();
                changed.insert(name.clone());
                let prefix = format!("{name}.");
                next.connections
                    .retain(|(f, t, _)| !f.starts_with(&prefix) && !t.starts_with(&prefix));
                let at_node = next.nodes.iter().position(|n| n.name == name).unwrap();
                let rank = next.nodes[at_node].rank;
                let type_name = g.pick_type();
                let mut node = g.node(name.clone(), type_name);
                node.rank = rank;
                next.nodes[at_node] = node.clone();
                g.wire(&mut next, &node);
                g.wire_sink(&mut next);
            }
            // Rewire: other sources for one instance, and for the sink.
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

    // Suspends and resumes of instances no swap changes, alternating.
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
        let s = g.rng.below(draw.frames as usize) as u64;
        edits.push(Edit {
            frame: s,
            instance: address(&name),
            resume: false,
        });
        if g.rng.chance(70) {
            let r = s + 1 + g.rng.below(2 * fade as usize + 8) as u64;
            if r < draw.frames {
                edits.push(Edit {
                    frame: r,
                    instance: address(&name),
                    resume: true,
                });
            }
        }
    }
    edits.sort_by_key(|e| e.frame);

    // External events at the initial graph's parameters and event inputs.
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
                frames: g.rng.below(65) as u64,
                curve: Curve::Linear,
            },
            _ => EventPayload::Trigger,
        };
        events.push(ExternalEvent {
            frame: g.rng.below(draw.frames as usize) as u64,
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
        frames: draw.frames,
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
    }
}

/// Three engine configurations for a case: block sizes, thread counts
/// (the first more than one) and partitions.
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
                *rng.pick(&[1usize, 2, 3, 4, 8]).unwrap()
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
    fn the_block_engine_matches_the_oracle(draw in draw()) {
        let registry = registry();
        let case = build(&registry, &draw);
        let oracle = signal_oracle::run(&registry, &case.graph, &case.script)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(oracle.channels, if draw.probe { PROBE_INPUTS } else { 2 });
        for setup in setups(draw.seed, draw.frames) {
            let engine = run_engine(&registry, &case.plan, &case.swaps, &case.script, &setup, &oracle);
            if let Err(difference) = compare(&oracle, &engine) {
                return Err(TestCaseError::fail(format!("{setup:?}: {difference}")));
            }
            if setup.threads > 1 {
                let stats = engine.stats;
                prop_assert!(stats.tasks >= 2, "{setup:?}: {stats:?}");
                prop_assert!(stats.parallel_segments > 0, "{setup:?}: {stats:?}");
                prop_assert!(stats.worker_tasks > 0, "{setup:?}: {stats:?}");
            }
        }
    }
}

/// The generator reaches what AC-GEN asks for, over the cases' seeds: every
/// standard type, feedback through `unit_delay` and `delay`, modulation,
/// event connections, events of every kind, edits and swaps.
#[test]
fn the_generator_covers_its_dimensions() {
    let registry = registry();
    let mut types = BTreeSet::new();
    let (mut feedback, mut modulation, mut event_links) = (0, 0, 0);
    let (mut sets, mut ramps, mut triggers, mut edits, mut swaps) = (0, 0, 0, 0, 0);
    for seed in 0..u64::from(CASES) {
        let draw = Draw {
            seed,
            instances: 2 + (seed % 11) as usize,
            frames: 512,
            rate: 1_000,
            probe: seed % 4 != 0,
            swaps: (seed % 3) as usize,
        };
        let case = build(&registry, &draw);
        for i in &case.plan.instances {
            types.insert(i.type_name);
        }
        let position: Vec<_> = case.plan.schedule.iter().collect();
        for (to, sources) in &case.plan.sources {
            for source in sources {
                let from = source.from.instance();
                let from_type = case
                    .plan
                    .instances
                    .iter()
                    .find(|i| i.address == from)
                    .unwrap()
                    .type_name;
                let at = |a: &InstanceAddress| position.iter().position(|p| *p == a);
                if matches!(from_type, "std.unit_delay" | "std.delay")
                    && at(&from) >= at(&to.instance())
                {
                    feedback += 1;
                }
                match source.shape {
                    PortShape::Events => event_links += 1,
                    _ => {
                        let target = case
                            .plan
                            .instances
                            .iter()
                            .find(|i| i.address == to.instance())
                            .unwrap();
                        if target
                            .descriptor
                            .parameters
                            .iter()
                            .any(|p| p.name == to.port())
                        {
                            modulation += 1;
                        }
                    }
                }
            }
        }
        for e in &case.script.events {
            match e.payload {
                EventPayload::Set(_) => sets += 1,
                EventPayload::Ramp { .. } => ramps += 1,
                EventPayload::Trigger => triggers += 1,
            }
        }
        edits += case.script.edits.len();
        swaps += case.script.swaps.len();
    }
    let all: BTreeSet<&str> = registry
        .iter()
        .map(|t| t.descriptor)
        .filter(|d| d.sink.is_none() && d.type_name.starts_with("std."))
        .filter(scalar)
        .map(|d| d.type_name)
        .collect();
    let missing: Vec<_> = all.difference(&types).collect();
    assert!(missing.is_empty(), "types never generated: {missing:?}");
    for (what, count) in [
        ("feedback", feedback),
        ("modulation", modulation),
        ("event connections", event_links),
        ("sets", sets),
        ("ramps", ramps),
        ("triggers", triggers),
        ("edits", edits),
        ("swaps", swaps),
    ] {
        assert!(count > 0, "the generator made no {what}");
    }
}

/// The property test's persistence reads every seed saved in the
/// regression file beside this source, in order.
#[test]
fn the_regression_file_is_found() {
    let saved: Vec<&str> = include_str!("generated.proptest-regressions")
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|seed| !seed.is_empty())
        .collect();
    assert!(!saved.is_empty(), "the regression file holds no seed");
    let loaded: Vec<String> = REGRESSIONS
        .load_persisted_failures2(None)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(loaded, saved);
}

/// A known failing seed, injected into a regression file of its own (never
/// the real one), is replayed before any new case and fails the run with
/// the value it draws.
#[test]
fn a_saved_seed_is_replayed_first() {
    let path = std::env::temp_dir().join(format!(
        "signal-oracle-replay-{}.proptest-regressions",
        std::process::id()
    ));
    std::fs::write(
        &path,
        "cc 5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed5eed # injected\n",
    )
    .unwrap();
    let path: &'static str = Box::leak(path.to_str().unwrap().to_owned().into_boxed_str());
    let config = |cases| ProptestConfig {
        cases,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(path))),
        ..ProptestConfig::default()
    };
    let seen = RefCell::new(Vec::new());

    // With no new cases, only a replay runs the test: the seed's value.
    TestRunner::new(config(0))
        .run(&draw(), |d| {
            seen.borrow_mut().push(d);
            Ok(())
        })
        .unwrap();
    let known = seen.take();
    assert_eq!(known.len(), 1, "the seed was not replayed: {known:?}");
    let known = known[0].clone();

    // The property fails on that value: the run stops at the replay.
    let result = TestRunner::new(config(CASES)).run(&draw(), |d| {
        seen.borrow_mut().push(d.clone());
        if d == known {
            Err(TestCaseError::fail("the known failing seed"))
        } else {
            Ok(())
        }
    });
    std::fs::remove_file(path).unwrap();
    match result {
        Err(TestError::Fail(_, value)) => assert_eq!(value, known),
        other => panic!("the known failing seed passed: {other:?}"),
    }
    assert_eq!(seen.borrow().first(), Some(&known), "a new case ran first");
}
