//! The per-frame executor allocates nothing after instantiation (AC1), measured
//! with the counting allocator of `signal-contract` (ADR 0017).

signal_contract::install_counting_allocator!();

mod common;

use common::{addr, conn, engine, ext, inst, inst_with, plan, port_addr, ramp, registry, scaled};
use signal_contract::alloc_counter::{self, Guard};
use signal_core::{EventPayload, Frame};
use signal_engine::LogRecord;

/// Every executor path runs inside the guard: external and module events,
/// triggers sent to a parameter, ramps (running, paused and pending),
/// modulation, a feedback loop, randomness, every suspension transition
/// (one-frame, fade-in interrupted, fade-out interrupted, long), discards, a NaN
/// fault and an overflow fault. Frame 20 is the maximal frame for `/c.in`:
/// X = 3 externals plus 16 events from each of two sources, cap = 35.
#[test]
fn executor_allocates_nothing_after_instantiation() {
    assert!(alloc_counter::is_installed());
    let plan = plan(
        vec![
            inst_with("f1", "test.flood", &[("count", 16.0)]),
            inst_with("f2", "test.flood", &[("count", 16.0)]),
            inst("c", "test.counter"),
            inst_with("over", "test.flood", &[("count", 17.0)]),
            inst("c2", "test.counter"),
            inst_with("s", "test.setter", &[("value", 0.25)]),
            inst_with("k", "test.const", &[("value", 0.125)]),
            inst("p", "test.probe_add"),
            inst_with("t", "test.trigger", &[("period", 2.0)]),
            inst("e", "test.event_delay"),
            inst("impulse", "test.impulse"),
            inst("loop", "test.gain"),
            inst("delay", "test.unit_delay"),
            inst("d", "test.dc"),
            inst("x", "test.faulty"),
            inst("n", "test.noise"),
            inst("rec", "test.recorder"),
        ],
        vec![
            conn("f1.out", "c.in"),
            conn("f2.out", "c.in"),
            conn("over.out", "c2.in"),
            conn("s.out", "p.value"),
            conn("k.out", "p.value"),
            conn("t.out", "p.value"),
            conn("t.out", "e.in"),
            conn("impulse.out", "loop.in"),
            scaled("delay.out", "loop.in", 0.5),
            conn("loop.out", "delay.in"),
            conn("d.out", "rec.zeta"),
            conn("x.out", "rec.zeta"),
            conn("n.out", "rec.alpha"),
            conn("loop.out", "rec.alpha"),
        ],
    );
    let external = vec![
        ext(20, 1, "/c.in", EventPayload::Trigger),
        ext(20, 2, "/c.in", EventPayload::Trigger),
        ext(20, 3, "/c.in", EventPayload::Trigger),
        ext(5, 4, "/p.value", ramp(1.0, 10)),
        ext(30, 5, "/p.value", EventPayload::Set(0.5)),
        ext(40, 6, "/p.value", ramp(0.0, 20)),
        ext(45, 7, "/p.value", ramp(0.75, 8)),
        ext(47, 8, "/c.in", EventPayload::Trigger),
    ];
    let mut engine = engine(&plan, 48_000, external);
    let edits = [
        ("/d", 40, 41),   // one-frame suspension
        ("/d", 100, 150), // a fade-out cut short
        ("/d", 200, 300), // a suspend during the fade-in, a resume during the fade-out
        ("/d", 400, 700), // longer than F = 240
        ("/p", 44, 50),   // a running ramp pauses; a ramp waits pending
        ("/c", 46, 48),   // discards
        ("/e", 60, 70),   // a state-output instance
    ];
    for (instance, suspend, resume) in edits {
        engine.suspend(&addr(instance), Frame(suspend)).unwrap();
        engine.resume(&addr(instance), Frame(resume)).unwrap();
    }
    let target = engine.port_id(&port_addr("/c.in")).unwrap();

    let mut maximal = 0;
    let guard = Guard::start();
    for _ in 0..1000 {
        engine.run_frame();
        if engine.next_frame() == Frame(21) {
            maximal = engine
                .frame_log()
                .iter()
                .filter(|r| matches!(r, LogRecord::Event { target: t, .. } if *t == target))
                .count();
        }
    }
    let counts = guard.counts();
    drop(guard);

    assert!(counts.is_zero(), "the executor allocated: {counts:?}");
    assert_eq!(maximal, 35);
    assert_eq!(engine.log_overflows(), 0);
    assert_eq!(engine.input_overflows(), 0);
    assert_eq!(engine.is_muted(&addr("/x")), Some(true));
    assert_eq!(engine.is_muted(&addr("/over")), Some(true));
    assert_eq!(engine.discards(&addr("/c")), Some(2 * 32 + 1));
    assert_eq!(engine.signal_output(&port_addr("/d.out")), Some(0.75));
}

/// M2-1 AC6: `commit_swap`, `push_external`, `push_suspend`/`push_resume`,
/// `run_frame` and `drain_log` allocate and free nothing, over several swaps
/// (an addition, a removal after its fade, a rewire, an identical plan), late
/// and in-advance events and suspends. Each swap is prepared, and each
/// interval's input built, between guarded intervals.
#[test]
fn swaps_and_pushed_input_allocate_nothing() {
    assert!(alloc_counter::is_installed());
    let base = || {
        vec![
            inst_with("o", "test.osc", &[("freq", 130.0)]),
            inst("d", "test.dc"),
            inst("pm", "test.probe_mul"),
            inst_with("t", "test.trigger", &[("period", 3.0)]),
            inst("c", "test.counter"),
            inst("rec", "test.recorder"),
        ]
    };
    let wiring = || {
        vec![
            conn("o.out", "rec.zeta"),
            conn("d.out", "rec.alpha"),
            conn("d.out", "pm.value"),
            conn("t.out", "c.in"),
        ]
    };
    let a = plan(base(), wiring());
    let mut with_noise = base();
    with_noise.push(inst("n", "test.noise"));
    let mut noise_wiring = wiring();
    noise_wiring.push(conn("n.out", "rec.alpha"));
    let b = plan(with_noise.clone(), noise_wiring);
    with_noise.retain(|i| i.name != "d");
    let c = plan(
        with_noise.clone(),
        vec![
            conn("o.out", "rec.zeta"),
            conn("n.out", "rec.alpha"),
            conn("t.out", "c.in"),
        ],
    );
    let d = plan(
        with_noise,
        vec![
            conn("n.out", "rec.zeta"),
            conn("o.out", "rec.alpha"),
            conn("o.out", "pm.value"),
            conn("t.out", "c.in"),
        ],
    );

    let mut engine = engine(&a, 1_000, vec![ext(3, 100, "/pm.value", ramp(0.25, 30))]);
    let removal = engine.removal_swap_frame(Frame(25)).0;
    // (interval end = commit frame, plan, events, suspends, resumes)
    let intervals = [
        (20, Some(&b), 0u64),
        (removal, Some(&c), 1),
        (50, Some(&d), 2),
        (70, Some(&d), 3),
        (100, None, 4),
    ];
    let mut old = Vec::new();
    for (end, next, k) in intervals {
        let start = engine.next_frame().0;
        let mut swap = next.map(|plan| engine.prepare_swap(plan, &registry()).unwrap());
        let mut events: Vec<_> = (start..end)
            .flat_map(|frame| {
                [
                    // Late (stamped 3 frames back), on time, and a trigger.
                    ext(
                        frame.saturating_sub(3),
                        1_000 * k + 2 * frame,
                        "/pm.value",
                        EventPayload::Set(0.5),
                    ),
                    ext(
                        frame,
                        1_000 * k + 2 * frame + 1,
                        "/c.in",
                        EventPayload::Trigger,
                    ),
                ]
            })
            .collect();
        let mut edits: Vec<_> = match k {
            1 => vec![(addr("/d"), 25, true)],
            3 => vec![
                (addr("/o"), 55, true),
                (addr("/o"), 62, false),
                (addr("/pm"), 50, true),
            ],
            4 => vec![(addr("/pm"), 75, false)],
            _ => vec![],
        };

        let guard = Guard::start();
        for frame in start..end {
            for event in events.drain(..2) {
                engine.push_external(event).unwrap();
            }
            for (instance, at, suspend) in edits.drain(..) {
                if suspend {
                    engine.push_suspend(instance, Frame(at)).unwrap();
                } else {
                    engine.push_resume(instance, Frame(at)).unwrap();
                }
            }
            engine.run_frame();
            engine.drain_log(&mut |_| {});
            assert_eq!(engine.next_frame(), Frame(frame + 1));
        }
        if let Some(swap) = swap.as_mut() {
            engine.commit_swap(swap, Frame(end)).unwrap();
        }
        let counts = guard.counts();
        drop(guard);
        assert!(counts.is_zero(), "interval {k} allocated: {counts:?}");

        old.extend(swap);
        drop(engine.reclaim_externals().collect::<Vec<_>>());
        drop(engine.reclaim_edits().collect::<Vec<_>>());
    }
    assert_eq!(engine.version(), 4);
    assert_eq!(engine.log_overflows(), 0);
    assert_eq!(engine.input_overflows(), 0);
    assert_eq!(engine.input_discards().unrouted, 0);
    assert_eq!(engine.is_suspended(&addr("/pm")), Some(false));
    assert!(old.iter().all(|swap| swap.is_committed()));
}
