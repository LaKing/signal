//! The interpreter. Section numbers are those of `docs/semantics.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use signal_core::{
    Combine, Curve, Direction, Event, EventPayload, EventSink, Feedthrough, Inputs,
    InstanceAddress, ModuleDescriptor, Outputs, PortAddress, PortKind, PortShape, PrepareContext,
    Primitive, Range, Registry, RngStream, SinkKind,
};

use crate::{
    DiscardReason, ExternalEvent, FaultReason, Graph, InstanceEnd, Output, ParameterEnd, Record,
    Script, Source,
};

/// Events an event output keeps per frame (§4).
const EVENTS_PER_FRAME: usize = 16;
/// The fault fade (§7).
const FAULT_FADE: u64 = 64;
/// The largest output magnitude (§7).
const MAGNITUDE: f64 = 1e6;

/// Why the oracle could not run a graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OracleError(pub String);

impl fmt::Display for OracleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "oracle: {}", self.0)
    }
}

impl std::error::Error for OracleError {}

fn error<T>(message: String) -> Result<T, OracleError> {
    Err(OracleError(message))
}

/// Runs `graph` for `script.frames` frames, one at a time.
pub fn run(registry: &Registry, graph: &Graph, script: &Script) -> Result<Output, OracleError> {
    let fade = fade_frames(script.sample_rate);
    let mut plan = Plan::build(registry, &with_converters(registry, graph)?, script, None)?;
    let mut output = Output {
        channels: plan.sink_inputs(),
        ..Output::default()
    };
    let mut version = 0;
    for n in 0..script.frames {
        let mut records = Vec::new();
        // Phase 1 (§2): a swap, then edits, then external events.
        if let Some(swap) = script.swaps.iter().find(|s| s.frame == n) {
            let graph = with_converters(registry, &swap.graph)?;
            plan = Plan::build(registry, &graph, script, Some(plan))?;
            version += 1;
            records.push(Record::Swap {
                frame: n,
                version,
                plan_version: swap.graph.version,
            });
            if plan.sink_inputs() != output.channels {
                return error("a swap changed the audio sink".to_owned());
            }
        }
        for edit in script.edits.iter().filter(|e| e.frame == n) {
            plan.edit(&edit.instance, edit.resume, n, &mut records);
        }
        let mut externals: Vec<&ExternalEvent> =
            script.events.iter().filter(|e| e.frame == n).collect();
        externals.sort_by_key(|e| e.sequence);
        plan.frame(n, &externals, fade, &mut records);
        output.samples.extend(plan.recorded());
        // §9: a swap, edits by instance, events and discards by target (in
        // delivery order), faults by instance.
        records.sort_by(|a, b| order(a).cmp(&order(b)));
        output.log.extend(records);
    }
    output.instances = plan.instance_ends();
    output.parameters = plan.parameter_ends();
    Ok(output)
}

/// A record's place in its frame's log (§9).
fn order(record: &Record) -> (u8, &str) {
    match record {
        Record::Swap { .. } => (0, ""),
        Record::Suspend { address, .. } | Record::Resume { address, .. } => (1, address.as_str()),
        Record::Event { target, .. } | Record::Discard { target, .. } => (2, target.as_str()),
        Record::Fault { address, .. } => (3, address.as_str()),
    }
}

/// The suspension fade length *F* (§6).
fn fade_frames(sample_rate: u32) -> u64 {
    ((u64::from(sample_rate) * 5 + 500) / 1000).max(1)
}

/// Frame `j` of a fade-out of `len` frames from `held` (§6, §7).
fn fade_out(held: f64, j: u64, len: u64) -> f64 {
    if j + 1 >= len {
        0.0
    } else {
        held * ((len - 1 - j) as f64 / len as f64)
    }
}

/// Frame `j` of a fade-in of `len` frames to `live` (§6).
fn fade_in(live: f64, j: u64, len: u64) -> f64 {
    if j + 1 >= len {
        live
    } else {
        live * ((j + 1) as f64 / len as f64)
    }
}

/// What the fault guard finds wrong with a value (§7).
fn fault_of(value: f64) -> Option<FaultReason> {
    if !value.is_finite() {
        Some(FaultReason::NonFinite)
    } else if value.abs() > MAGNITUDE {
        Some(FaultReason::Magnitude)
    } else {
        None
    }
}

/// How an instance takes part in a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Running,
    Suspended,
    Muted,
}

/// The suspension fade of an instance's `audio` outputs (§6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fade {
    None,
    /// Suspended: the next frame is frame `j` of the fade-out.
    Out(u64),
    /// Resumed or added: the next frame it runs is frame `j` of the fade-in.
    In(u64),
}

/// A parameter's base (§5): its value, a running ramp and a ramp held while
/// the instance is suspended.
#[derive(Clone, Copy, Debug)]
struct Base {
    value: f64,
    /// (from, to, frames, curve, frames run since it started)
    ramp: Option<(f64, f64, u64, Curve, u64)>,
    /// (to, frames, curve)
    pending: Option<(f64, u64, Curve)>,
}

impl Base {
    fn new(value: f64) -> Base {
        Base {
            value,
            ramp: None,
            pending: None,
        }
    }

    /// Step 1, on a frame the instance runs: a running ramp moves on; a ramp
    /// held while suspended starts from the value it reaches.
    fn advance(&mut self) {
        if let Some((from, to, frames, curve, ran)) = self.ramp {
            let ran = ran + 1;
            if ran >= frames {
                self.value = to;
                self.ramp = None;
            } else {
                self.value = from + (to - from) * curve.eval(ran as f64 / frames as f64);
                self.ramp = Some((from, to, frames, curve, ran));
            }
        }
        if let Some((to, frames, curve)) = self.pending.take() {
            self.ramp = Some((self.value, to, frames, curve, 0));
        }
    }

    /// Step 2: one event; false for a trigger, which a parameter ignores.
    /// While suspended a `set` acts at once and a `ramp` waits (§6).
    fn apply(&mut self, payload: EventPayload, suspended: bool) -> bool {
        match payload {
            EventPayload::Set(value)
            | EventPayload::Ramp {
                to: value,
                frames: 0,
                ..
            } => {
                self.value = value;
                self.ramp = None;
                self.pending = None;
            }
            EventPayload::Ramp { to, frames, curve } if suspended => {
                self.pending = Some((to, frames, curve));
            }
            EventPayload::Ramp { to, frames, curve } => {
                self.ramp = Some((self.value, to, frames, curve, 0));
            }
            EventPayload::Trigger => return false,
        }
        true
    }
}

/// The effective value (§5): base and modulation combined, then clamped.
fn effective(base: f64, modulation: Option<f64>, combine: Combine, range: Range) -> f64 {
    let value = match (modulation, combine) {
        (None, _) => base,
        (Some(m), Combine::Add) => base + m,
        (Some(m), Combine::Multiply) => base * m,
        (Some(m), Combine::Replace) => m,
    };
    if value < range.min {
        range.min
    } else if value > range.max {
        range.max
    } else {
        value
    }
}

/// The canonical sum of `scale · output`, from +0 (§3).
fn sum(sources: &[(usize, f64)], outputs: &[f64]) -> f64 {
    let mut total = 0.0;
    for &(o, scale) in sources {
        total += scale * outputs[o];
    }
    total
}

struct Param {
    port: PortAddress,
    base: Base,
    combine: Combine,
    range: Range,
    /// Signal sources: (output, scale), canonical order.
    modulation: Vec<(usize, f64)>,
    /// Event sources: (event output, connections), canonical order.
    events: Vec<(usize, usize)>,
    /// Modulation held from sources a swap removed (§12).
    held: Option<f64>,
    /// The effective value in the last frame the instance ran.
    effective: f64,
}

struct EventInput {
    port: PortAddress,
    /// (event output, connections), canonical order.
    sources: Vec<(usize, usize)>,
}

struct Inst {
    address: InstanceAddress,
    type_name: String,
    descriptor: ModuleDescriptor,
    primitive: Primitive,
    /// Per signal input: (output, scale), canonical order.
    signal_sources: Vec<Vec<(usize, f64)>>,
    /// This frame's signal input sums.
    signal_in: Vec<f64>,
    event_inputs: Vec<EventInput>,
    params: Vec<Param>,
    /// Its signal outputs (indices into `Plan::outputs`), and which are audio.
    outputs: Vec<usize>,
    audio: Vec<bool>,
    /// Per signal output: what a suspension or fault fades from.
    held: Vec<f64>,
    /// Its event outputs (indices into `Plan::emitted`) and their sinks.
    event_outputs: Vec<usize>,
    sinks: Vec<EventSink>,
    suspended: bool,
    /// Muted, with the frames since (the fault fade's position).
    fault: Option<u64>,
    fade: Fade,
    discards: u64,
    suspension_discards: u64,
    edited_at: Option<u64>,
    /// This frame's events on its event inputs, and where each input's end.
    events: Vec<Event>,
    ends: Vec<usize>,
    /// Its rate divisor (§16): it is called on frames that are multiples of
    /// it only.
    divisor: u64,
    /// Per signal output row: what it wrote at its last call (0 before),
    /// which its outputs show on the frames between calls (§16).
    live: Vec<f64>,
    /// Per signal output row: the output port it belongs to.
    row_ports: Vec<PortAddress>,
}

impl Inst {
    /// Whether it is called at frame `n` (§16).
    fn ticks_at(&self, n: u64) -> bool {
        n.is_multiple_of(self.divisor)
    }

    fn mode(&self) -> Mode {
        if self.fault.is_some() {
            Mode::Muted
        } else if self.suspended {
            Mode::Suspended
        } else {
            Mode::Running
        }
    }

    /// The first output port: signal (or vector), then event; none without
    /// outputs.
    fn first_port(&self) -> Option<PortAddress> {
        let event = self.ports(Direction::Out, PortShape::Events);
        self.row_ports.iter().cloned().chain(event).next()
    }

    fn ports(&self, direction: Direction, shape: PortShape) -> Vec<PortAddress> {
        self.descriptor
            .ports
            .iter()
            .filter(|p| p.direction == direction && p.shape == shape)
            .filter_map(|p| PortAddress::new(&self.address, p.name).ok())
            .collect()
    }
}

/// An instantiated graph and the values of its last frame.
struct Plan {
    /// In address order.
    instances: Vec<Inst>,
    /// Direct instances in an order that runs each source first.
    order: Vec<usize>,
    /// State-output instances.
    state_outputs: Vec<usize>,
    /// Every signal output's value as readers see it: of this frame once its
    /// owner has run, of the last frame before.
    outputs: Vec<f64>,
    output_ports: Vec<PortAddress>,
    output_owner: Vec<usize>,
    /// Every event output's events of this frame, in emission order, its
    /// address and its instance.
    emitted: Vec<Vec<EventPayload>>,
    event_ports: Vec<PortAddress>,
    event_owner: Vec<usize>,
    sink: Option<usize>,
    fade: u64,
}

impl Plan {
    /// Instantiates `graph`; after a swap from `old`, an instance with the
    /// same address and type survives with its state (§12).
    fn build(
        registry: &Registry,
        graph: &Graph,
        script: &Script,
        old: Option<Plan>,
    ) -> Result<Plan, OracleError> {
        let mut old = old;
        let mut sorted = graph.instances.clone();
        sorted.sort_by(|a, b| a.address.cmp(&b.address));

        let mut plan = Plan {
            instances: Vec::new(),
            order: Vec::new(),
            state_outputs: Vec::new(),
            outputs: Vec::new(),
            output_ports: Vec::new(),
            output_owner: Vec::new(),
            emitted: Vec::new(),
            event_ports: Vec::new(),
            event_owner: Vec::new(),
            sink: None,
            fade: fade_frames(script.sample_rate),
        };
        // Old instance index of each survivor.
        let mut survivors: Vec<Option<usize>> = Vec::new();
        for spec in &sorted {
            let Some(module_type) = registry.get(&spec.type_name) else {
                return error(format!("unknown type {}", spec.type_name));
            };
            let descriptor = module_type.descriptor;
            let survivor = old.as_ref().and_then(|old| {
                old.instances.iter().position(|o| {
                    o.address == spec.address
                        && o.type_name == spec.type_name
                        && o.descriptor == descriptor
                })
            });
            let primitive = match (survivor, old.as_mut()) {
                (Some(o), Some(old)) => {
                    // Its primitive moves in; a placeholder stays behind.
                    std::mem::replace(&mut old.instances[o].primitive, (module_type.create)())
                }
                _ => {
                    let mut primitive = (module_type.create)();
                    let context = PrepareContext {
                        sample_rate: f64::from(script.sample_rate),
                        max_frames: 1,
                        rng: RngStream::derive(script.seed, &spec.address),
                        lent: None,
                    };
                    match &mut primitive {
                        Primitive::Direct(module) => module.prepare(&context),
                        Primitive::StateOutput(module) => module.prepare(&context),
                    }
                    primitive
                }
            };
            let i = plan.instances.len();
            let mut inst = Inst {
                address: spec.address.clone(),
                type_name: spec.type_name.clone(),
                descriptor,
                primitive,
                signal_sources: Vec::new(),
                signal_in: Vec::new(),
                event_inputs: Vec::new(),
                params: Vec::new(),
                outputs: Vec::new(),
                audio: Vec::new(),
                held: Vec::new(),
                event_outputs: Vec::new(),
                sinks: Vec::new(),
                suspended: false,
                fault: None,
                fade: if old.is_some() {
                    Fade::In(0)
                } else {
                    Fade::None
                },
                discards: 0,
                suspension_discards: 0,
                edited_at: None,
                events: Vec::new(),
                ends: Vec::new(),
                divisor: rate_of(&descriptor)?,
                live: Vec::new(),
                row_ports: Vec::new(),
            };
            for port in descriptor.ports {
                let address = port_address(&spec.address, port.name)?;
                // A signal is one row, a vector of N is N (§16).
                let rows = match port.shape {
                    PortShape::Signal => 1,
                    PortShape::Vector(n) if n >= 1 => n as usize,
                    _ => 0,
                };
                match (port.direction, port.shape) {
                    (Direction::In, PortShape::Signal | PortShape::Vector(_)) if rows > 0 => {
                        for _ in 0..rows {
                            inst.signal_sources.push(Vec::new());
                            inst.signal_in.push(0.0);
                        }
                    }
                    (Direction::In, PortShape::Events) => inst.event_inputs.push(EventInput {
                        port: address,
                        sources: Vec::new(),
                    }),
                    (Direction::Out, PortShape::Signal | PortShape::Vector(_)) if rows > 0 => {
                        for _ in 0..rows {
                            inst.outputs.push(plan.outputs.len());
                            inst.audio.push(port.kind == PortKind::Audio);
                            inst.held.push(0.0);
                            inst.live.push(0.0);
                            inst.row_ports.push(address.clone());
                            plan.outputs.push(0.0);
                            plan.output_ports.push(address.clone());
                            plan.output_owner.push(i);
                        }
                    }
                    (Direction::Out, PortShape::Events) => {
                        inst.event_outputs.push(plan.emitted.len());
                        inst.sinks.push(EventSink::per_frame(EVENTS_PER_FRAME, 1));
                        plan.emitted.push(Vec::new());
                        plan.event_ports.push(address);
                        plan.event_owner.push(i);
                    }
                    _ => return error(format!("unsupported port {address}")),
                }
            }
            for p in descriptor.parameters {
                let initial = spec
                    .parameters
                    .iter()
                    .find(|(name, _)| name == p.name)
                    .map_or(p.default, |&(_, value)| value);
                inst.params.push(Param {
                    port: port_address(&spec.address, p.name)?,
                    base: Base::new(initial),
                    combine: p.combine,
                    range: p.range,
                    modulation: Vec::new(),
                    events: Vec::new(),
                    held: None,
                    effective: effective(initial, None, p.combine, p.range),
                });
            }
            if let (Some(o), Some(old)) = (survivor, old.as_ref()) {
                let before = &old.instances[o];
                inst.suspended = before.suspended;
                inst.fault = before.fault;
                inst.fade = before.fade;
                inst.discards = before.discards;
                inst.suspension_discards = before.suspension_discards;
                inst.edited_at = before.edited_at;
                inst.held = before.held.clone();
                inst.live = before.live.clone();
                for (&new, &was) in inst.outputs.iter().zip(&before.outputs) {
                    plan.outputs[new] = old.outputs[was];
                }
                for (param, was) in inst.params.iter_mut().zip(&before.params) {
                    param.base = was.base;
                    param.effective = was.effective;
                }
            }
            if descriptor.sink == Some(SinkKind::Audio) {
                if plan.sink.is_some() {
                    return error("several audio sinks".to_owned());
                }
                plan.sink = Some(i);
            }
            match descriptor.feedthrough {
                Feedthrough::Direct => {}
                Feedthrough::StateOutput => plan.state_outputs.push(i),
            }
            survivors.push(survivor);
            plan.instances.push(inst);
        }

        plan.connect(graph)?;
        plan.order = plan.direct_order()?;
        if let Some(old) = &old {
            plan.hold_modulation(old, &survivors);
        }
        Ok(plan)
    }

    /// Wires every connection, each input's sources in canonical order:
    /// byte-wise by source port address (ties by scale), §3 and §4.
    fn connect(&mut self, graph: &Graph) -> Result<(), OracleError> {
        let mut connections = graph.connections.clone();
        connections.sort_by(|a, b| {
            (&a.to, &a.from)
                .cmp(&(&b.to, &b.from))
                .then_with(|| a.scale.total_cmp(&b.scale))
        });
        for c in &connections {
            let signal = self.output_ports.iter().position(|p| *p == c.from);
            let event = self.event_ports.iter().position(|p| *p == c.from);
            let Some(inst) = self
                .instances
                .iter_mut()
                .find(|i| i.address == c.to.instance())
            else {
                return error(format!("no instance for {}", c.to));
            };
            let name = c.to.port();
            // The source's rows: all of its port's (a vector's elements).
            let source_rows = signal.map_or(0, |o| {
                self.output_ports[o..]
                    .iter()
                    .take_while(|p| **p == c.from)
                    .count()
            });
            let mut signal_inputs = 0;
            let mut event_inputs = 0;
            let mut found = false;
            for port in inst.descriptor.ports {
                if port.direction != Direction::In {
                    continue;
                }
                let here = port.name == name;
                let rows = match port.shape {
                    PortShape::Signal => 1,
                    PortShape::Vector(n) => n as usize,
                    _ => 0,
                };
                match (port.shape, here, signal, event) {
                    (PortShape::Signal | PortShape::Vector(_), true, Some(o), _)
                        if rows == source_rows =>
                    {
                        // Element by element (§16).
                        for e in 0..rows {
                            inst.signal_sources[signal_inputs + e].push((o + e, c.scale));
                        }
                        found = true;
                    }
                    (PortShape::Events, true, _, Some(e)) => {
                        add_event_source(&mut inst.event_inputs[event_inputs].sources, e);
                        found = true;
                    }
                    _ => {}
                }
                match port.shape {
                    PortShape::Events => event_inputs += 1,
                    _ => signal_inputs += rows,
                }
            }
            if let Some(param) = inst.params.iter_mut().find(|p| p.port == c.to) {
                match (signal, event) {
                    (Some(_), _) if source_rows != 1 => {
                        return error(format!("a vector into a parameter: {} -> {}", c.from, c.to));
                    }
                    (Some(o), _) => param.modulation.push((o, c.scale)),
                    (_, Some(e)) => add_event_source(&mut param.events, e),
                    _ => {}
                }
                found = true;
            }
            if !found || (signal.is_none() && event.is_none()) {
                return error(format!("invalid connection {} -> {}", c.from, c.to));
            }
        }
        Ok(())
    }

    /// The direct instances, each after the direct instances it reads (a
    /// state-output source's output is known before the frame, §2); ties by
    /// address. The order changes nothing (§2), so any valid one will do.
    fn direct_order(&self) -> Result<Vec<usize>, OracleError> {
        let n = self.instances.len();
        let direct: Vec<bool> = self
            .instances
            .iter()
            .map(|i| i.descriptor.feedthrough == Feedthrough::Direct)
            .collect();
        // reads[i]: the direct instances i reads.
        let mut reads: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, inst) in self.instances.iter().enumerate() {
            let signals = inst
                .signal_sources
                .iter()
                .flatten()
                .chain(inst.params.iter().flat_map(|p| &p.modulation))
                .map(|&(o, _)| self.output_owner[o]);
            let events = inst
                .event_inputs
                .iter()
                .flat_map(|e| &e.sources)
                .chain(inst.params.iter().flat_map(|p| &p.events))
                .map(|&(e, _)| self.event_owner[e]);
            for source in signals.chain(events) {
                if direct[source] {
                    reads[i].push(source);
                }
            }
        }
        let mut done = vec![false; n];
        let mut order = Vec::new();
        while order.len() < direct.iter().filter(|&&d| d).count() {
            let next = (0..n)
                .find(|&i| direct[i] && !done[i] && reads[i].iter().all(|&s| done[s] && s != i));
            let Some(i) = next else {
                return error("a cycle without a state-output instance".to_owned());
            };
            done[i] = true;
            order.push(i);
        }
        Ok(order)
    }

    /// §12: a surviving parameter left without signal sources holds the sum
    /// of what its removed sources output, or keeps a hold it had; a
    /// parameter that only lost sources still in the plan does not.
    fn hold_modulation(&mut self, old: &Plan, survivors: &[Option<usize>]) {
        let survived: Vec<usize> = survivors.iter().flatten().copied().collect();
        for (inst, survivor) in self.instances.iter_mut().zip(survivors) {
            let Some(o) = *survivor else { continue };
            for (param, was) in inst.params.iter_mut().zip(&old.instances[o].params) {
                if !param.modulation.is_empty() {
                    continue;
                }
                let removed: Vec<(usize, f64)> = was
                    .modulation
                    .iter()
                    .copied()
                    .filter(|&(out, _)| !survived.contains(&old.output_owner[out]))
                    .collect();
                param.held = if !removed.is_empty() {
                    Some(sum(&removed, &old.outputs))
                } else if was.modulation.is_empty() {
                    was.held
                } else {
                    None
                };
            }
        }
    }

    fn sink_inputs(&self) -> usize {
        self.sink.map_or(0, |s| self.instances[s].signal_in.len())
    }

    /// The audio sink's inputs this frame, each times its `gain` (§8).
    fn recorded(&self) -> Vec<f64> {
        let Some(s) = self.sink else {
            return Vec::new();
        };
        let sink = &self.instances[s];
        let gain = sink
            .params
            .iter()
            .find(|p| p.port.port() == "gain")
            .map(|p| p.effective);
        sink.signal_in
            .iter()
            .map(|&x| gain.map_or(x, |g| g * x))
            .collect()
    }

    /// A suspend or resume (§6), unless it does not fit the instance's state
    /// or the instance had one this frame already.
    fn edit(&mut self, address: &InstanceAddress, resume: bool, n: u64, records: &mut Vec<Record>) {
        let Some(inst) = self.instances.iter_mut().find(|i| i.address == *address) else {
            return;
        };
        if inst.suspended != resume || inst.edited_at == Some(n) {
            return;
        }
        inst.edited_at = Some(n);
        if resume {
            inst.suspended = false;
            if inst.fault.is_none() {
                inst.fade = Fade::In(0);
            }
            records.push(Record::Resume {
                frame: n,
                address: address.clone(),
                discarded: inst.suspension_discards,
            });
        } else {
            inst.suspended = true;
            inst.suspension_discards = 0;
            if inst.fault.is_none() {
                for (held, &o) in inst.held.iter_mut().zip(&inst.outputs) {
                    *held = self.outputs[o];
                }
                inst.fade = Fade::Out(0);
            }
            records.push(Record::Suspend {
                frame: n,
                address: address.clone(),
            });
        }
    }

    /// Phases 2–4 of frame `n` (§2).
    fn frame(
        &mut self,
        n: u64,
        externals: &[&ExternalEvent],
        fade: u64,
        records: &mut Vec<Record>,
    ) {
        self.fade = fade;
        for events in &mut self.emitted {
            events.clear();
        }
        for k in 0..self.state_outputs.len() {
            let i = self.state_outputs[k];
            self.emit(i, n, records);
        }
        for k in 0..self.order.len() {
            let i = self.order[k];
            self.process(i, n, externals, records);
        }
        for k in 0..self.state_outputs.len() {
            let i = self.state_outputs[k];
            self.absorb(i, n, externals, records);
        }
    }

    /// Instance `i`'s inputs for frame `n` (§3, §4, §5; §6 when suspended).
    fn gather(
        &mut self,
        i: usize,
        n: u64,
        externals: &[&ExternalEvent],
        records: &mut Vec<Record>,
    ) -> Mode {
        let inst = &mut self.instances[i];
        let mode = inst.mode();
        for (p, sources) in inst.signal_sources.iter().enumerate() {
            inst.signal_in[p] = sum(sources, &self.outputs);
        }

        let mut discards = 0;
        inst.events.clear();
        inst.ends.clear();
        for input in &inst.event_inputs {
            let mut arrivals: Vec<(EventPayload, Source)> = externals
                .iter()
                .filter(|e| e.target == input.port)
                .map(|e| (e.payload, Source::External(e.sequence)))
                .collect();
            for &(e, copies) in &input.sources {
                for &payload in &self.emitted[e] {
                    for _ in 0..copies {
                        arrivals.push((payload, Source::Port(self.event_ports[e].clone())));
                    }
                }
            }
            for (payload, source) in arrivals {
                let (frame, target) = (n, input.port.clone());
                records.push(match mode {
                    Mode::Running => {
                        inst.events.push(Event { offset: 0, payload });
                        Record::Event {
                            frame,
                            target,
                            payload,
                            source,
                        }
                    }
                    Mode::Suspended | Mode::Muted => {
                        discards += 1;
                        Record::Discard {
                            frame,
                            target,
                            payload,
                            source,
                            reason: if mode == Mode::Muted {
                                DiscardReason::Muted
                            } else {
                                DiscardReason::Suspended
                            },
                        }
                    }
                });
            }
            inst.ends.push(inst.events.len());
        }

        for param in &mut inst.params {
            if mode == Mode::Running {
                param.base.advance();
            }
            let mut arrivals: Vec<(EventPayload, Source)> = externals
                .iter()
                .filter(|e| e.target == param.port)
                .map(|e| (e.payload, Source::External(e.sequence)))
                .collect();
            for &(e, copies) in &param.events {
                for &payload in &self.emitted[e] {
                    for _ in 0..copies {
                        arrivals.push((payload, Source::Port(self.event_ports[e].clone())));
                    }
                }
            }
            for (payload, source) in arrivals {
                let applied = match mode {
                    Mode::Muted => None,
                    _ => Some(param.base.apply(payload, mode == Mode::Suspended)),
                };
                let is_set = matches!(
                    payload,
                    EventPayload::Set(_) | EventPayload::Ramp { frames: 0, .. }
                );
                if applied == Some(true) && is_set {
                    param.held = None;
                }
                let reason = match (applied, mode) {
                    (Some(true), _) => None,
                    (None, _) => Some(DiscardReason::Muted),
                    (Some(false), Mode::Suspended) => Some(DiscardReason::Suspended),
                    (Some(false), _) => Some(DiscardReason::NotAParameterEvent),
                };
                let (frame, target) = (n, param.port.clone());
                records.push(match reason {
                    None => Record::Event {
                        frame,
                        target,
                        payload,
                        source,
                    },
                    Some(reason) => {
                        discards += 1;
                        Record::Discard {
                            frame,
                            target,
                            payload,
                            source,
                            reason,
                        }
                    }
                });
            }
            if mode == Mode::Running {
                let modulation = if param.modulation.is_empty() {
                    param.held
                } else {
                    Some(sum(&param.modulation, &self.outputs))
                };
                param.effective =
                    effective(param.base.value, modulation, param.combine, param.range);
            }
        }

        inst.discards += discards;
        if mode == Mode::Suspended {
            inst.suspension_discards += discards;
        }
        mode
    }

    /// A direct instance's frame `n` (phase 3).
    fn process(
        &mut self,
        i: usize,
        n: u64,
        externals: &[&ExternalEvent],
        records: &mut Vec<Record>,
    ) {
        match self.gather(i, n, externals, records) {
            // Between its ticks it shows what it wrote last (§16).
            Mode::Running if !self.instances[i].ticks_at(n) => {
                let live = self.instances[i].live.clone();
                self.commit(i, n, true, &live, records);
            }
            Mode::Running => {
                let inst = &mut self.instances[i];
                let mut outputs = vec![0.0; inst.outputs.len()];
                for sink in &mut inst.sinks {
                    sink.start_call(1);
                }
                let params: Vec<f64> = inst.params.iter().map(|p| p.effective).collect();
                let inputs = Inputs::new(1, &inst.signal_in, &params, &inst.events, &inst.ends);
                let mut out = Outputs::new(1, &mut outputs, &mut inst.sinks);
                let ok = match &mut inst.primitive {
                    Primitive::Direct(module) => {
                        catch_unwind(AssertUnwindSafe(|| module.process(&inputs, &mut out))).is_ok()
                    }
                    Primitive::StateOutput(_) => true,
                };
                self.commit(i, n, ok, &outputs, records);
            }
            Mode::Suspended => self.fade_suspended(i),
            Mode::Muted => self.fade_muted(i),
        }
    }

    /// A state-output instance's emit at frame `n` (phase 2).
    fn emit(&mut self, i: usize, n: u64, records: &mut Vec<Record>) {
        match self.instances[i].mode() {
            Mode::Running if !self.instances[i].ticks_at(n) => {
                let live = self.instances[i].live.clone();
                self.commit(i, n, true, &live, records);
            }
            Mode::Running => {
                let inst = &mut self.instances[i];
                let mut outputs = vec![0.0; inst.outputs.len()];
                for sink in &mut inst.sinks {
                    sink.start_call(1);
                }
                let mut out = Outputs::new(1, &mut outputs, &mut inst.sinks);
                let ok = match &mut inst.primitive {
                    Primitive::StateOutput(module) => {
                        catch_unwind(AssertUnwindSafe(|| module.emit(&mut out))).is_ok()
                    }
                    Primitive::Direct(_) => true,
                };
                self.commit(i, n, ok, &outputs, records);
            }
            Mode::Suspended => self.fade_suspended(i),
            Mode::Muted => self.fade_muted(i),
        }
    }

    /// A state-output instance's absorb at frame `n` (phase 4). A panic
    /// here mutes it from the next frame; this frame's outputs stand (§14).
    fn absorb(
        &mut self,
        i: usize,
        n: u64,
        externals: &[&ExternalEvent],
        records: &mut Vec<Record>,
    ) {
        if self.gather(i, n, externals, records) != Mode::Running || !self.instances[i].ticks_at(n)
        {
            return;
        }
        let inst = &mut self.instances[i];
        let params: Vec<f64> = inst.params.iter().map(|p| p.effective).collect();
        let inputs = Inputs::new(1, &inst.signal_in, &params, &inst.events, &inst.ends);
        let ok = match &mut inst.primitive {
            Primitive::StateOutput(module) => {
                catch_unwind(AssertUnwindSafe(|| module.absorb(&inputs))).is_ok()
            }
            Primitive::Direct(_) => true,
        };
        if !ok {
            for (held, &o) in inst.held.iter_mut().zip(&inst.outputs) {
                *held = self.outputs[o];
            }
            inst.fault = Some(0);
            records.push(Record::Fault {
                frame: n,
                address: inst.address.clone(),
                port: inst.first_port(),
                reason: FaultReason::Panic,
            });
        }
    }

    /// After a call of instance `i` at frame `n` that wrote `live` (`ok`:
    /// it did not panic): the fault guard (§7), then the outputs become
    /// visible, `audio` ones through a fade-in, and its events are kept.
    fn commit(&mut self, i: usize, n: u64, ok: bool, live: &[f64], records: &mut Vec<Record>) {
        let fault = if ok {
            self.check(i, live)
        } else {
            Some((self.instances[i].first_port(), FaultReason::Panic))
        };
        let inst = &mut self.instances[i];
        if let Some((port, reason)) = fault {
            // Muted from this frame, fading from what readers saw before it.
            for (held, &o) in inst.held.iter_mut().zip(&inst.outputs) {
                *held = self.outputs[o];
            }
            inst.fault = Some(0);
            records.push(Record::Fault {
                frame: n,
                address: inst.address.clone(),
                port,
                reason,
            });
            self.fade_muted(i);
            return;
        }
        for (p, &o) in inst.outputs.iter().enumerate() {
            self.outputs[o] = match inst.fade {
                Fade::In(j) if inst.audio[p] => fade_in(live[p], j, self.fade),
                _ => live[p],
            };
        }
        if let Fade::In(j) = inst.fade {
            inst.fade = if j + 1 >= self.fade {
                Fade::None
            } else {
                Fade::In(j + 1)
            };
        }
        inst.live.copy_from_slice(live);
        for (sink, &e) in inst.sinks.iter().zip(&inst.event_outputs) {
            self.emitted[e] = sink.events().iter().map(|event| event.payload).collect();
        }
    }

    /// The fault guard's check of one frame's outputs: signal outputs, then
    /// event outputs, in descriptor order (§7).
    fn check(&self, i: usize, live: &[f64]) -> Option<(Option<PortAddress>, FaultReason)> {
        let inst = &self.instances[i];
        for (p, &value) in live.iter().enumerate() {
            if let Some(reason) = fault_of(value) {
                return Some((inst.row_ports.get(p).cloned(), reason));
            }
        }
        let events = inst.ports(Direction::Out, PortShape::Events);
        for (k, sink) in inst.sinks.iter().enumerate() {
            let port = events.get(k).cloned();
            if sink.first_overflow().is_some() {
                return Some((port, FaultReason::EventOverflow));
            }
            for event in sink.events() {
                let value = match event.payload {
                    EventPayload::Set(value) | EventPayload::Ramp { to: value, .. } => value,
                    EventPayload::Trigger => continue,
                };
                if let Some(reason) = fault_of(value) {
                    return Some((port, reason));
                }
            }
        }
        None
    }

    /// A suspended instance's outputs: `audio` ones fade out, the others hold
    /// (§6).
    fn fade_suspended(&mut self, i: usize) {
        let inst = &mut self.instances[i];
        if let Fade::Out(j) = inst.fade {
            for (p, &o) in inst.outputs.iter().enumerate() {
                if inst.audio[p] {
                    self.outputs[o] = fade_out(inst.held[p], j, self.fade);
                }
            }
            inst.fade = Fade::Out(j + 1);
        }
    }

    /// A muted instance's outputs: the 64-frame fault fade (§7).
    fn fade_muted(&mut self, i: usize) {
        let inst = &mut self.instances[i];
        if let Some(j) = inst.fault {
            for (p, &o) in inst.outputs.iter().enumerate() {
                self.outputs[o] = fade_out(inst.held[p], j, FAULT_FADE);
            }
            inst.fault = Some(j + 1);
        }
    }

    fn instance_ends(&self) -> Vec<InstanceEnd> {
        self.instances
            .iter()
            .map(|i| InstanceEnd {
                address: i.address.clone(),
                suspended: i.suspended,
                muted: i.fault.is_some(),
                discards: i.discards,
            })
            .collect()
    }

    fn parameter_ends(&self) -> Vec<ParameterEnd> {
        let mut ends: BTreeMap<PortAddress, ParameterEnd> = BTreeMap::new();
        for param in self.instances.iter().flat_map(|i| &i.params) {
            ends.insert(
                param.port.clone(),
                ParameterEnd {
                    port: param.port.clone(),
                    base: param.base.value,
                    effective: param.effective,
                },
            );
        }
        ends.into_values().collect()
    }
}

/// An event source of an input: duplicate connections from one output
/// deliver each of its events once per connection, in a row.
fn add_event_source(sources: &mut Vec<(usize, usize)>, e: usize) {
    match sources.iter_mut().find(|(s, _)| *s == e) {
        Some((_, copies)) => *copies += 1,
        None => sources.push((e, 1)),
    }
}

fn port_address(instance: &InstanceAddress, name: &str) -> Result<PortAddress, OracleError> {
    PortAddress::new(instance, name).map_err(|e| OracleError(format!("{instance}.{name}: {e}")))
}

/// An instance's rate divisor (§16): the one its outputs, event ports and
/// parameters share (its signal inputs may differ), 1 without them.
fn rate_of(descriptor: &ModuleDescriptor) -> Result<u64, OracleError> {
    let own = descriptor
        .ports
        .iter()
        .filter(|p| p.direction == Direction::Out || p.shape == PortShape::Events)
        .map(|p| p.rate.get())
        .chain(descriptor.parameters.iter().map(|p| p.rate.get()));
    let mut rate = None;
    for d in own {
        if rate.is_some_and(|r| r != d) {
            return error(format!("{}: mixed rate divisors", descriptor.type_name));
        }
        rate = Some(d);
    }
    Ok(u64::from(rate.unwrap_or(1)))
}

/// A port kind's name in a converter's type name (§16).
fn kind_name(kind: PortKind) -> Result<&'static str, OracleError> {
    Ok(match kind {
        PortKind::Audio => "audio",
        PortKind::Control => "control",
        PortKind::Gate => "gate",
        PortKind::Trigger => "trigger",
        PortKind::Frequency => "frequency",
        PortKind::Gain => "gain",
        PortKind::Phase => "phase",
        PortKind::Generic => "generic",
        _ => return error(format!("no converter name for kind {kind:?}")),
    })
}

/// `graph` with its implicit converters (§16), inserted here from the rule
/// as `docs/semantics.md` states it: a connection from a signal or vector
/// output at divisor *s* into an input of the same shape (or a signal into a
/// parameter) at *t* ≠ *s*, one a multiple of the other, runs through the
/// instance `<source instance>/<port>_d<t>` of type
/// `core.<hold|decimate>.d<s>.d<t>.<signal|vectorN>.<kind>` (hold when *s* is
/// the multiple), shared by every target of that source port at *t*: the
/// source feeds its `in` with scale 1, its `out` feeds the target with the
/// connection's scale. The type itself is the registry's.
pub(crate) fn with_converters(registry: &Registry, graph: &Graph) -> Result<Graph, OracleError> {
    let descriptor = |address: &InstanceAddress| {
        let instance = graph.instances.iter().find(|i| i.address == *address)?;
        Some(registry.get(&instance.type_name)?.descriptor)
    };
    let mut out = Graph {
        version: graph.version,
        instances: graph.instances.clone(),
        connections: Vec::new(),
    };
    let mut made: Vec<(PortAddress, u32)> = Vec::new();
    for c in &graph.connections {
        let (from, to) = (descriptor(&c.from.instance()), descriptor(&c.to.instance()));
        let source = from.and_then(|d| {
            d.ports
                .iter()
                .find(|p| p.name == c.from.port() && p.direction == Direction::Out)
                .copied()
        });
        let target = to.and_then(|d| {
            d.ports
                .iter()
                .find(|p| p.name == c.to.port() && p.direction == Direction::In)
                .map(|p| (p.shape, p.rate.get()))
                .or_else(|| {
                    d.parameters
                        .iter()
                        .find(|p| p.name == c.to.port())
                        .map(|p| (PortShape::Signal, p.rate.get()))
                })
        });
        let (Some(source), Some((shape, t))) = (source, target) else {
            out.connections.push(c.clone());
            continue;
        };
        let s = source.rate.get();
        let word = if s != t && s % t == 0 {
            "hold"
        } else if s != t && t % s == 0 {
            "decimate"
        } else {
            out.connections.push(c.clone());
            continue;
        };
        let shape = match (source.shape, shape) {
            (PortShape::Signal, PortShape::Signal) => "signal".to_owned(),
            (PortShape::Vector(n), PortShape::Vector(m)) if n == m && n >= 1 => {
                format!("vector{n}")
            }
            _ => {
                out.connections.push(c.clone());
                continue;
            }
        };
        let name = format!("{}_d{t}", c.from.port());
        let address = c
            .from
            .instance()
            .child(&name)
            .map_err(|e| OracleError(format!("converter {name}: {e}")))?;
        if !made.contains(&(c.from.clone(), t)) {
            made.push((c.from.clone(), t));
            out.instances.push(crate::Instance {
                address: address.clone(),
                type_name: format!("core.{word}.d{s}.d{t}.{shape}.{}", kind_name(source.kind)?),
                parameters: Vec::new(),
            });
            out.connections.push(crate::Connection {
                from: c.from.clone(),
                to: port_address(&address, "in")?,
                scale: 1.0,
            });
        }
        out.connections.push(crate::Connection {
            from: port_address(&address, "out")?,
            to: c.to.clone(),
            scale: c.scale,
        });
    }
    Ok(out)
}
