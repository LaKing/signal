//! Instantiation (AC1), the frame phases (AC2), refused inputs and edits, and
//! the fault guard (AC6).

mod common;

use common::{
    addr, conn, engine, engine_seeded, ext, inst, inst_with, output_trace, plan, port_addr,
    registry,
};
use signal_core::{
    EventPayload, Frame, MAX_VECTOR_WIDTH, PortDescriptor, PortShape, Registry, RngStream,
};
use signal_engine::{Engine, EngineConfig, EngineError, MAX_PREPARED_BYTES, MAX_RESERVED_BYTES};
use signal_output::render;

/// Each primitive is prepared with the sample rate, the block capacity and
/// `RngStream::derive(seed, address)`.
#[test]
fn prepare_gets_rate_k1_and_derived_rng_stream() {
    let plan = plan(
        vec![inst("ctx", "test.context"), inst("noise", "test.noise")],
        vec![],
    );
    let mut engine = engine_seeded(&plan, 44_100, 7, vec![]);
    let mut stream = RngStream::derive(7, &addr("/noise"));
    let (rate, frames, noise) = (
        port_addr("/ctx.rate"),
        port_addr("/ctx.frames"),
        port_addr("/noise.out"),
    );
    for _ in 0..16 {
        engine.run_frame();
        assert_eq!(engine.signal_output(&rate), Some(44_100.0));
        assert_eq!(
            engine.signal_output(&frames),
            Some(EngineConfig::DEFAULT_BLOCK_CAPACITY as f64)
        );
        let expected = stream.next_f64();
        assert_eq!(
            engine.signal_output(&noise).map(f64::to_bits),
            Some(expected.to_bits())
        );
    }
}

/// y[n] = gain[n] · (impulse[n] + delay[n]), delay[n] = y[n − 1], with an external
/// `set(0.5)` on the gain at frame 3. It shows external input before process
/// (the set acts at frame 3), emit before process (the gain reads the delay's
/// frame-n output), and absorb after process (the delay's output at n + 1 is
/// the gain's output at n).
#[test]
fn frame_phases_run_in_adr_order() {
    let plan = plan(
        vec![
            inst("impulse", "test.impulse"),
            inst("loop", "test.gain"),
            inst("delay", "test.unit_delay"),
        ],
        vec![
            conn("impulse.out", "loop.in"),
            conn("delay.out", "loop.in"),
            conn("loop.out", "delay.in"),
        ],
    );
    let external = vec![ext(3, 1, "/loop.gain", EventPayload::Set(0.5))];
    let mut engine = engine(&plan, 48_000, external);
    let (y, d) = (port_addr("/loop.out"), port_addr("/delay.out"));
    let mut ys = Vec::new();
    let mut ds = Vec::new();
    for _ in 0..6 {
        engine.run_frame();
        ys.push(engine.signal_output(&y).unwrap());
        ds.push(engine.signal_output(&d).unwrap());
    }
    assert_eq!(ys, [1.0, 1.0, 1.0, 0.5, 0.25, 0.125]);
    assert_eq!(ds, [0.0, 1.0, 1.0, 1.0, 0.5, 0.25]);
}

/// Bad external events, plans the registry cannot build, and edits out of turn
/// are refused before anything runs.
#[test]
fn instantiation_errors() {
    let plan = plan(
        vec![
            inst("t", "test.trigger"),
            inst("c", "test.counter"),
            inst("g", "test.gain"),
        ],
        vec![conn("t.out", "c.in")],
    );
    let config = EngineConfig::new(48_000, 1);
    let refused = |config, external| Engine::new(&plan, &registry(), config, external).err();

    let silent = EngineConfig {
        sample_rate: 0,
        ..config
    };
    assert_eq!(
        refused(silent, vec![]),
        Some(EngineError::InvalidSampleRate)
    );
    assert_eq!(
        Engine::new(&plan, &Registry::new(), config, vec![]).err(),
        Some(EngineError::UnknownModuleType {
            instance: addr("/c"),
            type_name: "test.counter"
        })
    );
    assert_eq!(
        refused(config, vec![ext(0, 1, "/nope.in", EventPayload::Trigger)]),
        Some(EngineError::UnknownTarget {
            target: port_addr("/nope.in")
        })
    );
    for target in ["/g.in", "/c.out"] {
        assert_eq!(
            refused(config, vec![ext(0, 1, target, EventPayload::Set(1.0))]),
            Some(EngineError::NotAnEventTarget {
                target: port_addr(target)
            })
        );
    }
    assert_eq!(
        refused(
            config,
            vec![ext(0, 1, "/g.gain", EventPayload::Set(f64::NAN))]
        ),
        Some(EngineError::NonFiniteValue { sequence: 1 })
    );
    let twice = vec![
        ext(0, 5, "/g.gain", EventPayload::Set(1.0)),
        ext(1, 5, "/c.in", EventPayload::Trigger),
    ];
    assert_eq!(
        refused(config, twice),
        Some(EngineError::DuplicateSequence { sequence: 5 })
    );

    let mut engine = Engine::new(&plan, &registry(), config, vec![]).unwrap();
    let c = addr("/c");
    assert_eq!(
        engine.suspend(&addr("/nope"), Frame(1)),
        Err(EngineError::UnknownInstance {
            instance: addr("/nope")
        })
    );
    let invalid = |frame| {
        Err(EngineError::InvalidEdit {
            instance: addr("/c"),
            frame: Frame(frame),
        })
    };
    assert_eq!(engine.resume(&c, Frame(3)), invalid(3));
    assert_eq!(engine.suspend(&c, Frame(3)), Ok(()));
    assert_eq!(engine.suspend(&c, Frame(5)), invalid(5));
    assert_eq!(engine.resume(&c, Frame(3)), invalid(3));
    assert_eq!(engine.resume(&c, Frame(4)), Ok(()));
    for _ in 0..6 {
        engine.run_frame();
    }
    assert_eq!(
        engine.suspend(&c, Frame(2)),
        Err(EngineError::FrameInPast {
            frame: Frame(2),
            next: Frame(6)
        })
    );
}

/// A fixture emitting NaN from frame 8 is faded to 0 over 64 frames from its
/// last output, muted and logged; the other instance and the render go on.
#[test]
fn nan_output_is_faded_muted_and_logged() {
    let plan = plan(
        vec![
            inst("f", "test.faulty"),
            inst_with("o", "test.osc", &[("freq", 187.5)]),
            inst("rec", "test.recorder"),
        ],
        vec![conn("f.out", "rec.zeta"), conn("o.out", "rec.alpha")],
    );
    let mut engine = engine(&plan, 48_000, vec![]);
    let rendered = render(&mut engine, 100).unwrap();
    assert_eq!(rendered.samples.len(), 200);
    assert!(rendered.samples.iter().all(|x| x.is_finite()));
    let zeta: Vec<f64> = rendered.samples.chunks(2).map(|f| f[0]).collect();
    let alpha: Vec<f64> = rendered.samples.chunks(2).map(|f| f[1]).collect();
    for (n, &x) in zeta.iter().enumerate() {
        let expected = match n {
            0..8 => 0.5,
            8..71 => 0.5 * ((71 - n) as f64 / 64.0),
            _ => 0.0,
        };
        assert_eq!(x.to_bits(), expected.to_bits(), "frame {n}");
    }
    for (n, &x) in alpha.iter().enumerate() {
        assert_eq!(x, (n % 256) as f64 / 256.0, "frame {n}");
    }
    assert_eq!(engine.is_muted(&addr("/f")), Some(true));
    let log = String::from_utf8(rendered.log).unwrap();
    let faults: Vec<&str> = log
        .lines()
        .filter(|l| l.contains(r#""type":"fault""#))
        .collect();
    assert_eq!(
        faults,
        [r#"{"type":"fault","frame":8,"address":"/f","port":"/f.out","reason":"non_finite"}"#]
    );
}

#[test]
fn magnitude_over_1e6_is_a_fault() {
    let plan = plan(
        vec![inst("l", "test.loud"), inst("rec", "test.recorder")],
        vec![conn("l.out", "rec.zeta")],
    );
    let mut engine = engine(&plan, 48_000, vec![]);
    let rendered = render(&mut engine, 20).unwrap();
    assert_eq!(rendered.samples[16], 0.5 * (63.0 / 64.0));
    let log = String::from_utf8(rendered.log).unwrap();
    assert_eq!(
        log.lines()
            .filter(|l| l.contains(r#""type":"fault""#))
            .collect::<Vec<_>>(),
        [r#"{"type":"fault","frame":8,"address":"/l","port":"/l.out","reason":"magnitude"}"#]
    );
    assert_eq!(engine.is_muted(&addr("/l")), Some(true));
}

/// 17 events in one frame overflow a sink: a fault. Its events are not
/// delivered; a source of exactly 16 is fine.
#[test]
fn event_sink_overflow_is_a_fault() {
    let plan = plan(
        vec![
            inst_with("f", "test.flood", &[("count", 17.0)]),
            inst_with("ok", "test.flood", &[("count", 16.0)]),
            inst("c", "test.counter"),
        ],
        vec![conn("f.out", "c.in"), conn("ok.out", "c.in")],
    );
    let mut engine = engine(&plan, 48_000, vec![]);
    let counts = output_trace(&mut engine, 2, "/c.out");
    assert_eq!(counts, [16.0, 32.0]);
    assert_eq!(engine.is_muted(&addr("/f")), Some(true));
    assert_eq!(engine.is_muted(&addr("/ok")), Some(false));

    let mut engine = common::engine(&plan, 48_000, vec![]);
    engine.run_frame();
    let fault = engine
        .frame_log()
        .iter()
        .filter_map(|r| engine.log_line(r))
        .map(|line| serde_json::to_string(&line).unwrap())
        .find(|line| line.contains("fault"));
    assert_eq!(
        fault.as_deref(),
        Some(
            r#"{"type":"fault","frame":0,"address":"/f","port":"/f.out","reason":"event_overflow"}"#
        )
    );
}

/// Audit P1 (2026-10-15, `docs/semantics.md` §16): a plan is refused with
/// a named reason, before any instance is created or buffer allocated, when
/// a vector is wider than `MAX_VECTOR_WIDTH` (a plan made without the graph
/// compiler's check) or its buffers would exceed their budgets at the block
/// capacity asked for; the widest vector runs at the default capacity.
#[test]
fn plans_beyond_the_memory_limits_are_refused_before_allocating() {
    let widest = format!("core.hold.d2.d1.vector{MAX_VECTOR_WIDTH}.generic");
    let trigger = plan(vec![inst("t", "test.trigger")], vec![]);
    let plan = plan(vec![inst("h", &widest)], vec![]);
    let config = EngineConfig::new(48_000, 1);
    assert!(Engine::new(&plan, &registry(), config, vec![]).is_ok());

    // 2³² − 1 elements: 8 TiB for one 256-frame buffer.
    let mut huge = plan.clone();
    let mut descriptor = huge.instances[0].descriptor;
    descriptor.ports = Box::leak(
        descriptor
            .ports
            .iter()
            .map(|p| PortDescriptor {
                shape: PortShape::Vector(u32::MAX),
                ..*p
            })
            .collect::<Box<[_]>>(),
    );
    huge.instances[0].descriptor = descriptor;
    assert_eq!(
        Engine::new(&huge, &registry(), config, vec![]).err(),
        Some(EngineError::VectorTooWide {
            port: port_addr("/h.in"),
            width: u32::MAX,
        })
    );

    let at = |block_capacity| EngineConfig {
        block_capacity,
        ..config
    };
    // Rows of values: 8192 of them, and as many again for one call, at
    // 2²⁰ frames are about 128 GiB.
    let refused = Engine::new(&plan, &registry(), at(1 << 20), vec![]).err();
    assert!(
        matches!(
            refused,
            Some(EngineError::PlanTooLarge { what: "value buffers", bytes: Some(b), budget, .. })
                if b > budget && budget == MAX_PREPARED_BYTES
        ),
        "{refused:?}"
    );
    let overflow = Engine::new(&plan, &registry(), at(usize::MAX), vec![]).err();
    assert!(
        matches!(
            overflow,
            Some(EngineError::PlanTooLarge {
                bytes: None,
                capacity: usize::MAX,
                ..
            })
        ),
        "{overflow:?}"
    );
    // One event output: its few values fit at 2²⁶ frames, its room for 16
    // events per frame (and the log's) does not.
    let refused = Engine::new(&trigger, &registry(), at(1 << 26), vec![]).err();
    assert!(
        matches!(
            refused,
            Some(EngineError::PlanTooLarge { what: "buffers in all", bytes: Some(b), budget, .. })
                if b > budget && budget == MAX_RESERVED_BYTES
        ),
        "{refused:?}"
    );
    let message = refused.unwrap().to_string();
    assert!(
        message
            .starts_with("the plan's buffers in all at a block capacity of 67108864 frames take "),
        "{message}"
    );
}
