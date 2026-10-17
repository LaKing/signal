//! Instance tables (ADR 0028 §4, M2.11-1 AC4/AC5), domain-free: the plan
//! carries an instance's table, every call hands it over, and a swap that
//! changes it reaches the kept instance at the swap frame without resetting
//! its state, allocating nothing on the calling thread (the counting
//! allocator, ADR 0017), bit-identically for every block size and thread
//! count.

signal_contract::install_counting_allocator!();

mod common;

use common::{config_threads, inst, registry};
use signal_contract::alloc_counter::{self, Guard};
use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Frame, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor, Registry, Table,
    fnv1a64,
};
use signal_engine::Engine;
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, build_plan};

const fn output(name: &'static str) -> PortDescriptor {
    PortDescriptor {
        name,
        direction: Direction::Out,
        shape: PortShape::Signal,
        rate: RateDivisor::ONE,
        kind: PortKind::Control,
        unit: None,
        range: None,
    }
}

/// `test.table_reader`: counts the frames it ran (`pos`, its state) and
/// outputs the first value of row `pos mod rows` of its table (`value`, 0
/// without one).
const READER: ModuleDescriptor = ModuleDescriptor {
    type_name: "test.table_reader",
    doc: "fixture: reads its table",
    ports: &[output("pos"), output("value")],
    parameters: &[],
    feedthrough: Feedthrough::Direct,
    sink: None,
    latency: None,
    reproducibility: signal_core::Reproducibility::BitExact,
    category: None,
};

#[derive(Default)]
struct Reader {
    pos: u64,
}

impl DirectPrimitive for Reader {
    fn describe() -> ModuleDescriptor {
        READER
    }

    fn prepare(&mut self, _ctx: &PrepareContext) {
        self.pos = 0;
    }

    fn reset(&mut self) {
        self.pos = 0;
    }

    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        for n in 0..inputs.frames() {
            let value = inputs.table().map_or(0.0, |table| {
                let rows = table.rows().max(1) as u64;
                table
                    .row(usize::try_from(self.pos % rows).unwrap_or(0))
                    .first()
                    .copied()
                    .unwrap_or(0.0)
            });
            if let Some(out) = outputs.signal(0).get_mut(n) {
                *out = self.pos as f64;
            }
            if let Some(out) = outputs.signal(1).get_mut(n) {
                *out = value;
            }
            self.pos += 1;
        }
    }
}

fn tables_registry() -> Registry {
    let mut registry = registry();
    registry
        .register(ModuleType {
            descriptor: READER,
            create: || Primitive::Direct(Box::new(Reader::default())),
        })
        .unwrap();
    registry
}

fn table(values: &[f64]) -> Table {
    Table::new(1, values.to_vec()).unwrap()
}

/// Readers `r0 … r{n−1}`, each through a `test.gain`, with the tables given
/// (`None`: no table).
fn plan(tables: &[Option<Table>]) -> ExecutionPlan {
    let mut instances = Vec::new();
    let mut connections = Vec::new();
    for i in 0..tables.len() {
        instances.push(inst(&format!("r{i}"), "test.table_reader"));
        instances.push(inst(&format!("g{i}"), "test.gain"));
        connections.push(ConnectionDef {
            from: format!("r{i}.value"),
            to: format!("g{i}.in"),
            scale: None,
        });
    }
    let root = CompositeDef {
        name: "project".to_owned(),
        instances,
        connections,
        exports: Exports::default(),
    };
    let mut plan = build_plan(&root, &[], &tables_registry(), 1).unwrap();
    for instance in &mut plan.instances {
        let name = instance.address.to_string();
        if let Some(i) = name
            .strip_prefix("/r")
            .and_then(|i| i.parse::<usize>().ok())
        {
            instance.table = tables[i].clone();
        }
    }
    plan
}

/// The `pos` and `value` of `/r{i}` at every frame of `frames`, swapping to
/// `next` at frame `at`, in blocks of `k` on `threads` threads.
fn run(
    first: &ExecutionPlan,
    next: &ExecutionPlan,
    at: u64,
    frames: u64,
    k: usize,
    threads: usize,
) -> Vec<Vec<(f64, f64)>> {
    let registry = tables_registry();
    let config = config_threads(1_000, 7, threads);
    let mut engine = Engine::new(first, &registry, config, Vec::new()).unwrap();
    let readers = first
        .instances
        .iter()
        .filter(|i| i.type_name == "test.table_reader")
        .count();
    let mut traces = vec![Vec::new(); readers];
    let mut swap = Some(engine.prepare_swap(next, &registry).unwrap());
    while engine.next_frame().0 < frames {
        let now = engine.next_frame().0;
        if now == at
            && let Some(mut prepared) = swap.take()
        {
            engine.drain_log(&mut |_| {});
            engine.commit_swap(&mut prepared, Frame(at)).unwrap();
        }
        let limit = if now < at { at } else { frames };
        let len = k.min(usize::try_from(limit - now).unwrap());
        assert_eq!(engine.run_block(len), len, "{:?}", engine.failure());
        for (i, trace) in traces.iter_mut().enumerate() {
            let pos = engine
                .signal_output_index(&format!("/r{i}.pos").parse().unwrap())
                .unwrap();
            let value = engine
                .signal_output_index(&format!("/r{i}.value").parse().unwrap())
                .unwrap();
            for f in 0..len {
                trace.push((
                    engine.signal_output_at(pos, f).unwrap(),
                    engine.signal_output_at(value, f).unwrap(),
                ));
            }
        }
        engine.drain_log(&mut |_| {});
    }
    traces
}

/// The table reaches its instance; a swap that changes it reaches the kept
/// instance at the swap frame, and its state (`pos`) goes on.
#[test]
fn a_changed_table_reaches_the_kept_instance_at_the_swap_frame() {
    let first = plan(&[Some(table(&[1.0, 2.0, 3.0])), None]);
    let next = plan(&[Some(table(&[10.0, 20.0])), Some(table(&[5.0]))]);
    let traces = run(&first, &next, 10, 16, 4, 1);
    let positions: Vec<f64> = traces[0].iter().map(|t| t.0).collect();
    assert_eq!(positions, (0..16).map(|n| n as f64).collect::<Vec<_>>());
    let values: Vec<f64> = traces[0].iter().map(|t| t.1).collect();
    assert_eq!(
        values,
        [
            1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0, // the first table
            10.0, 20.0, 10.0, 20.0, 10.0, 20.0, // from frame 10, at pos 10…
        ]
    );
    // An instance given a table by the swap reads it from the swap frame on.
    let second: Vec<f64> = traces[1].iter().map(|t| t.1).collect();
    assert_eq!(second[..10], [0.0; 10]);
    assert_eq!(second[10..], [5.0; 6]);
    assert_eq!(traces[1].last().unwrap().0, 15.0);
}

/// Committing a swap that changes a table, and running the frames around it,
/// allocate and free nothing on the calling thread.
#[test]
fn a_table_swap_allocates_nothing() {
    assert!(alloc_counter::is_installed());
    let registry = tables_registry();
    let tables = |base: f64| -> Vec<Option<Table>> {
        (0..3)
            .map(|i| Some(table(&[base + i as f64, base, base * 2.0])))
            .collect()
    };
    let plans = [plan(&tables(1.0)), plan(&tables(7.0)), plan(&tables(3.0))];
    let config = config_threads(1_000, 7, 1);
    let mut engine = Engine::new(&plans[0], &registry, config, Vec::new()).unwrap();
    let mut old = Vec::new();
    for (step, next) in plans[1..].iter().enumerate() {
        let at = 100 * (step as u64 + 1);
        let mut swap = engine.prepare_swap(next, &registry).unwrap();
        let guard = Guard::start();
        while engine.next_frame().0 < at {
            let len = 64.min(usize::try_from(at - engine.next_frame().0).unwrap());
            engine.run_block(len);
            engine.drain_log(&mut |_| {});
        }
        engine.commit_swap(&mut swap, Frame(at)).unwrap();
        engine.run_block(64);
        engine.drain_log(&mut |_| {});
        let counts = guard.counts();
        drop(guard);
        assert!(counts.is_zero(), "swap {step} allocated: {counts:?}");
        old.push(swap);
    }
    assert_eq!(engine.version(), 2);
    let pos = engine
        .signal_output_index(&"/r0.pos".parse().unwrap())
        .unwrap();
    assert_eq!(engine.signal_output_at(pos, 63), Some(263.0));
}

/// Every block size and thread count gives the same output, a table swap
/// included (ADR 0019 §1, §2).
#[test]
fn tables_are_bit_identical_in_blocks_and_on_threads() {
    let first = plan(&[
        Some(table(&[1.0, -2.0, 0.5])),
        Some(table(&[3.0; 7])),
        None,
        Some(table(&[0.25, 0.125])),
    ]);
    let next = plan(&[
        Some(table(&[9.0, 8.0])),
        Some(table(&[3.0; 7])),
        Some(table(&[4.0, 5.0, 6.0, 7.0])),
        None,
    ]);
    let hash = |traces: &[Vec<(f64, f64)>]| {
        let bytes: Vec<u8> = traces
            .iter()
            .flatten()
            .flat_map(|(p, v)| [p.to_bits(), v.to_bits()])
            .flat_map(u64::to_le_bytes)
            .collect();
        fnv1a64(&bytes)
    };
    let reference = hash(&run(&first, &next, 333, 1_000, 1, 1));
    for k in [1, 64, 256] {
        for threads in [1, 2, 4, 8] {
            assert_eq!(
                hash(&run(&first, &next, 333, 1_000, k, threads)),
                reference,
                "k = {k}, threads = {threads}"
            );
        }
    }
}
