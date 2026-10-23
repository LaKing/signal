//! The comparison harness (M2.7-6): runs a [`Script`] through the block
//! engine and compares what it recorded, logged and ended with against the
//! oracle, bit for bit. The engine's plans come from `signal-graph`; the
//! oracle's graph is built apart from them (by the generator) or converted
//! from a plan here, with its connections shuffled, so that the oracle
//! depends on no order the plan gives.

#![allow(dead_code)]

pub mod rates;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_core::{EventPayload, Frame, InstanceAddress, PortAddress, Registry, SplitMix64};
use signal_engine::{Engine, EngineConfig, HookPoint, Schedule, ScheduleStats};
use signal_graph::ExecutionPlan;
use signal_oracle::{
    Connection, DiscardReason, FaultReason, Graph, Instance, InstanceEnd, Output, ParameterEnd,
    Record, Script, Source,
};
use signal_output::render_blocks;

/// The standard primitives and `oracle.probe`.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    signal_oracle::register_probe(&mut registry).unwrap();
    registry
}

/// `items` in an order drawn from `seed`.
pub fn shuffled<T>(mut items: Vec<T>, seed: u64) -> Vec<T> {
    let mut rng = SplitMix64::new(seed);
    for i in (1..items.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
    items
}

/// The oracle's graph of a flattened plan: its instances and connections,
/// shuffled.
pub fn graph_of(plan: &ExecutionPlan, seed: u64) -> Graph {
    let instances = plan
        .instances
        .iter()
        .map(|i| Instance {
            address: i.address.clone(),
            type_name: i.type_name.to_owned(),
            parameters: i.parameters.iter().map(|(k, &v)| (k.clone(), v)).collect(),
            stream: i.stream.clone(),
        })
        .collect();
    let connections = plan
        .sources
        .iter()
        .flat_map(|(to, sources)| {
            sources.iter().map(|s| Connection {
                from: s.from.clone(),
                to: to.clone(),
                scale: s.scale,
            })
        })
        .collect();
    Graph {
        version: plan.version,
        instances: shuffled(instances, seed),
        connections: shuffled(connections, seed ^ 0x9e37_79b9),
    }
}

/// How the engine runs a script.
#[derive(Clone, Debug)]
pub struct Setup {
    /// Its block size.
    pub block: usize,
    /// Its threads; more than one runs every segment on the workers
    /// (`Schedule::FORCED`).
    pub threads: usize,
    /// Frames before which a block must end, besides the swaps': a
    /// partition of the run.
    pub splits: Vec<u64>,
}

/// What the engine recorded, logged and ended with.
#[derive(Debug)]
pub struct EngineRun {
    pub samples: Vec<f64>,
    pub log: Vec<Value>,
    pub parameters: Vec<ParameterEnd>,
    pub instances: Vec<InstanceEnd>,
    pub stats: ScheduleStats,
}

/// Whether the hook holds the caller (once per render).
static HOLD: AtomicBool = AtomicBool::new(false);
/// Tasks the workers have started.
static WORKER_TASKS: AtomicU64 = AtomicU64::new(0);

/// The task hook: a worker starting a task is counted; while armed, the
/// caller entering a job waits (at most 2 s, yielding) until a worker has
/// started one, then disarms. It changes when tasks run, never what they
/// compute (`docs/semantics.md` §13: the thread count changes nothing), and
/// makes `worker_tasks > 0` hold whatever the timing.
fn hold_until_a_worker_runs(point: HookPoint) {
    match point {
        HookPoint::Task { lane, .. } if lane > 0 => {
            WORKER_TASKS.fetch_add(1, Ordering::SeqCst);
        }
        HookPoint::Enter { lane: 0 } if HOLD.load(Ordering::SeqCst) => {
            let before = WORKER_TASKS.load(Ordering::SeqCst);
            let deadline = Instant::now() + Duration::from_secs(2);
            while WORKER_TASKS.load(Ordering::SeqCst) == before && Instant::now() < deadline {
                std::thread::yield_now();
            }
            HOLD.store(false, Ordering::SeqCst);
        }
        _ => {}
    }
}

/// Runs `script` from `plan` (and `swaps`, the engine's plans of the
/// script's swaps, in order) through the engine as `setup` says. The ports
/// and instances whose end state is read are the oracle's (`expected`).
pub fn run_engine(
    registry: &Registry,
    plan: &ExecutionPlan,
    swaps: &[ExecutionPlan],
    script: &Script,
    setup: &Setup,
    expected: &Output,
) -> EngineRun {
    let config = EngineConfig {
        threads: setup.threads,
        schedule: if setup.threads > 1 {
            Schedule::FORCED
        } else {
            Schedule::DEFAULT
        },
        ..EngineConfig::new(script.sample_rate, script.seed)
    };
    let externals = script
        .events
        .iter()
        .map(|e| signal_engine::ExternalEvent {
            frame: Frame(e.frame),
            sequence: e.sequence,
            target: e.target.clone(),
            payload: e.payload,
        })
        .collect();
    let mut engine = Engine::new(plan, registry, config, externals).unwrap();
    for edit in &script.edits {
        let scheduled = if edit.resume {
            engine.resume(&edit.instance, Frame(edit.frame))
        } else {
            engine.suspend(&edit.instance, Frame(edit.frame))
        };
        scheduled.unwrap();
    }
    if setup.threads > 1 {
        engine.set_task_hook(Some(hold_until_a_worker_runs));
        HOLD.store(true, Ordering::SeqCst);
    }

    let mut boundaries: Vec<u64> = script
        .swaps
        .iter()
        .map(|s| s.frame)
        .chain(setup.splits.iter().copied())
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    let rendered = render_blocks(&mut engine, script.frames, setup.block, |engine| {
        let next = engine.next_frame().0;
        if let Some(k) = script.swaps.iter().position(|s| s.frame == next) {
            let mut prepared = engine.prepare_swap(&swaps[k], registry)?;
            engine.commit_swap(&mut prepared, Frame(next))?;
        }
        Ok(boundaries.iter().find(|&&b| b > next).map(|&b| Frame(b)))
    })
    .unwrap();
    HOLD.store(false, Ordering::SeqCst);

    let log = String::from_utf8(rendered.log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let parameters = expected
        .parameters
        .iter()
        .map(|p| {
            let value = engine.parameter(&p.port);
            ParameterEnd {
                port: p.port.clone(),
                base: value.map_or(f64::NAN, |v| v.base),
                effective: value.map_or(f64::NAN, |v| v.effective),
            }
        })
        .collect();
    let instances = expected
        .instances
        .iter()
        .map(|i| InstanceEnd {
            address: i.address.clone(),
            suspended: engine.is_suspended(&i.address).unwrap_or(false),
            muted: engine.is_muted(&i.address).unwrap_or(false),
            discards: engine.discards(&i.address).unwrap_or(u64::MAX),
        })
        .collect();
    EngineRun {
        samples: rendered.samples,
        log,
        parameters,
        instances,
        stats: engine.schedule_stats(),
    }
}

/// Where the engine first differs from the oracle, if it does.
pub fn compare(oracle: &Output, engine: &EngineRun) -> Result<(), String> {
    let channels = oracle.channels.max(1);
    if oracle.samples.len() != engine.samples.len() {
        return Err(format!(
            "samples: oracle {} values, engine {}",
            oracle.samples.len(),
            engine.samples.len()
        ));
    }
    for (k, (a, b)) in oracle.samples.iter().zip(&engine.samples).enumerate() {
        if a.to_bits() != b.to_bits() {
            return Err(format!(
                "frame {} channel {}: oracle {a:?}, engine {b:?}",
                k / channels,
                k % channels
            ));
        }
    }
    let lines: Vec<Value> = oracle.log.iter().map(record_json).collect();
    for (k, (a, b)) in lines.iter().zip(&engine.log).enumerate() {
        if a != b {
            return Err(format!("log line {k}: oracle {a}, engine {b}"));
        }
    }
    if lines.len() != engine.log.len() {
        return Err(format!(
            "log: oracle {} lines, engine {}; first extra: {:?}",
            lines.len(),
            engine.log.len(),
            lines.get(engine.log.len()).or(engine.log.get(lines.len()))
        ));
    }
    for (a, b) in oracle.parameters.iter().zip(&engine.parameters) {
        if a.base.to_bits() != b.base.to_bits() || a.effective.to_bits() != b.effective.to_bits() {
            return Err(format!("parameter {}: oracle {a:?}, engine {b:?}", a.port));
        }
    }
    for (a, b) in oracle.instances.iter().zip(&engine.instances) {
        if a != b {
            return Err(format!(
                "instance {}: oracle {a:?}, engine {b:?}",
                a.address
            ));
        }
    }
    Ok(())
}

/// A record as its `log.jsonl` line (`docs/semantics.md` §9).
pub fn record_json(record: &Record) -> Value {
    match record {
        Record::Swap {
            frame,
            version,
            plan_version,
        } => {
            json!({"type": "swap", "frame": frame, "version": version, "plan_version": plan_version})
        }
        Record::Event {
            frame,
            target,
            payload,
            source,
        } => json!({
            "type": "event", "frame": frame, "target": target.as_str(),
            "payload": payload_json(*payload), "source": source_json(source),
        }),
        Record::Discard {
            frame,
            target,
            payload,
            source,
            reason,
        } => json!({
            "type": "discard", "frame": frame, "target": target.as_str(),
            "payload": payload_json(*payload), "source": source_json(source),
            "reason": match reason {
                DiscardReason::Suspended => "suspended",
                DiscardReason::Muted => "muted",
                DiscardReason::NotAParameterEvent => "not_a_parameter_event",
            },
        }),
        Record::Suspend { frame, address } => {
            json!({"type": "suspend", "frame": frame, "address": address.as_str()})
        }
        Record::Resume {
            frame,
            address,
            discarded,
        } => {
            json!({"type": "resume", "frame": frame, "address": address.as_str(), "discarded": discarded})
        }
        Record::Fault {
            frame,
            address,
            port,
            reason,
        } => {
            let mut line = json!({
                "type": "fault", "frame": frame, "address": address.as_str(),
                "reason": match reason {
                    FaultReason::NonFinite => "non_finite",
                    FaultReason::Magnitude => "magnitude",
                    FaultReason::EventOverflow => "event_overflow",
                    FaultReason::Panic => "panic",
                },
            });
            if let (Some(port), Some(fields)) = (port, line.as_object_mut()) {
                fields.insert("port".to_owned(), json!(port.as_str()));
            }
            line
        }
    }
}

fn payload_json(payload: EventPayload) -> Value {
    match payload {
        EventPayload::Trigger => json!("trigger"),
        EventPayload::Set(value) => json!({"set": value}),
        EventPayload::Ramp { to, frames, .. } => {
            json!({"ramp": {"to": to, "frames": frames, "curve": "linear"}})
        }
    }
}

fn source_json(source: &Source) -> Value {
    match source {
        Source::External(sequence) => json!({"external": {"sequence": sequence}}),
        Source::Port(port) => json!({"port": port.as_str()}),
    }
}

/// An instance address.
pub fn addr(text: &str) -> InstanceAddress {
    text.parse().unwrap()
}

/// A port address.
pub fn port(text: &str) -> PortAddress {
    text.parse().unwrap()
}
