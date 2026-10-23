//! M2.8-1 AC2: with scope streams active, `Runner::process` allocates and
//! frees nothing, measured with the counting allocator of `signal-contract`
//! (ADR 0017) on the audio thread only, as V11 does: a guard around each
//! call on the thread that runs the Runner, while the control thread starts,
//! restarts and stops scopes (their ports go to the audio thread and come
//! back) and drains their blocks.

signal_contract::install_counting_allocator!();

use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use signal_contract::alloc_counter::{self, Counts, Guard};
use signal_core::{PortAddress, Registry};
use signal_engine::RealtimeConfig;
use signal_graph::{CompositeDef, ConnectionDef, Exports, InstanceDef, build_plan};
use signal_output::{CallbackSizes, realtime};

const RATE: u32 = 48_000;
const CALLBACKS: u64 = 6_000;
const MOST_FRAMES: usize = 512;
const CHANNELS: usize = 2;

#[test]
fn runner_process_with_scopes_allocates_and_frees_nothing() {
    assert!(alloc_counter::is_installed());
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    let instance = |name: &str, module: &str, freq: Option<f64>| InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: freq
            .map(|f| ("freq".to_owned(), f))
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        scope: None,
    };
    let connection = |from: &str, to: &str| ConnectionDef {
        from: from.to_owned(),
        to: to.to_owned(),
        scale: None,
    };
    let root = CompositeDef {
        name: "project".to_owned(),
        instances: vec![
            instance("a", "std.sine_osc", Some(440.0)),
            instance("b", "std.sine_osc", Some(220.0)),
            instance("out", "std.audio_out", None),
        ],
        connections: vec![
            connection("a.out", "out.l"),
            connection("b.out", "out.l"),
            connection("a.out", "out.r"),
        ],
        exports: Exports::default(),
    };
    let plan = build_plan(&root, &[], &registry, 0).unwrap();
    let mut config = RealtimeConfig::new(RATE, 1);
    config.engine.threads = 2;
    let (mut runner, mut controller) = realtime(&plan, &registry, config).unwrap();
    let a: PortAddress = "/a.out".parse().unwrap();
    let b: PortAddress = "/b.out".parse().unwrap();
    controller.scope_start(0, a.clone(), 1).unwrap();
    controller.scope_start(1, b.clone(), 7).unwrap();

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

    let mut blocks = 0u64;
    let mut step = 0u64;
    while !audio.is_finished() {
        controller.poll(&mut ());
        controller.poll_scopes(|_| blocks += 1);
        step += 1;
        // Restart and stop slots while the runner plays: each command
        // carries a port to the audio thread and brings the old one back.
        match step % 40 {
            10 => drop(controller.scope_start(2, a.clone(), 3)),
            20 => drop(controller.scope_stop(2)),
            30 => drop(controller.scope_start(1, b.clone(), 5)),
            _ => {}
        }
        thread::sleep(Duration::from_micros(200));
    }
    let (runner, total) = audio.join().unwrap();
    controller.poll_scopes(|_| blocks += 1);
    assert_eq!(
        total,
        Counts::default(),
        "heap calls inside Runner::process"
    );
    assert!(blocks > 0, "the scopes delivered blocks");
    drop(runner);
}
