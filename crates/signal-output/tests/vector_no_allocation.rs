//! M2.8-7 (C9): with vector streams active on the visual-rate physics,
//! `Runner::process` allocates and frees nothing, on two threads, measured
//! with the counting allocator of `signal-contract` (ADR 0017) around each
//! call on the audio thread, as for scopes: the control thread starts,
//! restarts and stops vector slots (their ports go to the audio thread and
//! come back) and drains their snapshots. The plan runs the event bridge
//! (`std.count`) into `std.mesh2d`, `std.particles` and `std.body2d`.

signal_contract::install_counting_allocator!();

use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_core::{PortAddress, Registry};
use signal_engine::RealtimeConfig;
use signal_graph::{CompositeDef, ConnectionDef, Exports, InstanceDef, build_plan};
use signal_output::{CallbackSizes, VECTOR_SLOTS, realtime};

const RATE: u32 = 48_000;
const CALLBACKS: u64 = 3_000;
const MOST_FRAMES: usize = 512;
const CHANNELS: usize = 2;

#[test]
fn runner_process_with_vector_streams_allocates_and_frees_nothing() {
    assert!(alloc_counter::is_installed());
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let instance = |name: &str, module: &str, parameters: &[(&str, f64)]| InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: parameters
            .iter()
            .map(|&(k, v)| (k.to_owned(), v))
            .collect::<BTreeMap<_, _>>(),
    };
    let connection = |from: &str, to: &str| ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    };
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            instance("clock", "std.transport", &[("bpm", 400.0)]),
            instance("hits", "std.count", &[]),
            instance("membrane", "std.mesh2d", &[]),
            instance("sparks", "std.particles", &[("rate", 300.0)]),
            instance("bodies", "std.body2d", &[]),
            instance("tone", "std.sine_osc", &[("freq", 220.0)]),
            instance("out", "std.audio_out", &[]),
        ],
        connections: vec![
            connection("clock.beat", "hits.in"),
            connection("hits.window", "membrane.strike"),
            connection("hits.window", "sparks.impulse"),
            connection("hits.window", "bodies.kick"),
            connection("tone.out", "out.l"),
        ],
        exports: Exports::default(),
    };
    let plan = build_plan(&root, &[], &registry, 0).unwrap();
    let mut config = RealtimeConfig::new(RATE, 1);
    config.engine.threads = 2;
    let (mut runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let port = |text: &str| -> PortAddress { text.parse().unwrap() };
    let streams = [
        (port("/membrane.z"), 256),
        (port("/sparks.pos"), 128),
        (port("/sparks.life"), 64),
        (port("/bodies.bodies"), 24),
        (port("/hits.window"), signal_std::BRIDGE_ROWS),
    ];
    for (slot, (p, rows)) in streams.iter().enumerate() {
        // The window runs at divisor 1, the visual types at 800 (and
        // `bodies` is as wide as a reading).
        let decimation = if p.instance().as_str() == "/hits" {
            1
        } else {
            800
        };
        controller
            .vector_start(&plan, slot, p.clone(), *rows, decimation)
            .unwrap();
    }
    assert!(streams.len() < VECTOR_SLOTS);

    let audio = thread::spawn(move || {
        let mut sizes = CallbackSizes::new(9);
        let mut buffer = vec![0.0f32; MOST_FRAMES * CHANNELS];
        let mut total = Counts::default();
        for _ in 0..CALLBACKS {
            let frames = sizes.next_size().min(MOST_FRAMES);
            let out = &mut buffer[..frames * CHANNELS];
            let guard = Guard::start();
            runner.process(out, CHANNELS);
            let counts = guard.counts();
            drop(guard);
            total.allocations += counts.allocations;
            total.deallocations += counts.deallocations;
        }
        // The guard counts this thread: one allocation shows.
        let guard = Guard::start();
        drop(std::hint::black_box(Box::new(1u8)));
        assert_eq!(guard.counts().allocations, 1);
        drop(guard);
        (runner, total)
    });

    let mut snapshots = 0u64;
    let mut step = 0u64;
    while !audio.is_finished() {
        controller.poll(&mut ());
        controller.poll_vectors(|_| snapshots += 1);
        step += 1;
        match step % 40 {
            10 => drop(controller.vector_start(&plan, 6, streams[0].0.clone(), 256, 1600)),
            20 => drop(controller.vector_stop(6)),
            30 => drop(controller.vector_start(&plan, 1, streams[1].0.clone(), 128, 2400)),
            _ => {}
        }
        thread::sleep(Duration::from_micros(200));
    }
    let (runner, total) = audio.join().unwrap();
    controller.poll_vectors(|_| snapshots += 1);
    assert_eq!(
        total,
        Counts::default(),
        "heap calls inside Runner::process"
    );
    assert!(snapshots > 0, "the vector slots delivered snapshots");
    drop(runner);
}
