//! M2.7-9 (the re-review's stuck primitive): hang detection, not recovery.
//! `test.stall` is a test hook: a primitive whose `process` blocks for 3
//! device periods (and then until the test has seen the hang reported, at
//! most 5 s, so that detection never races the stall's end). Under the
//! threaded null driver the Controller counts the callback once it has been
//! seen running for `hang_periods` periods, and reports it with the
//! instance still running, `/stall`: on the calling thread alone (one
//! thread, the path every k = 1 or small segment takes) and on a worker
//! (two threads, every segment forced onto the workers). A run without a
//! stall reports none.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use signal_core::{
    DirectPrimitive, Direction, Feedthrough, Inputs, ModuleDescriptor, ModuleType, Outputs,
    PortDescriptor, PortKind, PortShape, PrepareContext, Primitive, RateDivisor, Registry,
};
use signal_engine::{ControlSink, HangLine, RealtimeConfig, Schedule};
use signal_graph::{CompositeDef, ConnectionDef, ExecutionPlan, Exports, InstanceDef, build_plan};
use signal_output::{Driver, NullDriver, realtime};

const RATE: u32 = 48_000;
/// 50 ms periods.
const BUFFER: u64 = 2_400;
const PERIOD: Duration = Duration::from_millis(50);
const HANG_PERIODS: u32 = 2;

/// The next `process` of `test.stall` blocks.
static ARMED: AtomicBool = AtomicBool::new(false);
/// The test has seen the hang: the blocked call may return.
static RELEASED: AtomicBool = AtomicBool::new(false);

const fn port(name: &'static str, direction: Direction) -> PortDescriptor {
    PortDescriptor {
        name,
        direction,
        shape: PortShape::Signal,
        rate: RateDivisor::ONE,
        kind: PortKind::Audio,
        unit: None,
        range: None,
    }
}

const STALL_PORTS: &[PortDescriptor] = &[port("in", Direction::In), port("out", Direction::Out)];

/// `out` = `in`; armed, one call blocks for 3 periods and then until
/// released (at most 5 s).
#[derive(Default)]
struct Stall;

impl DirectPrimitive for Stall {
    fn describe() -> ModuleDescriptor {
        ModuleDescriptor {
            type_name: "test.stall",
            doc: "a test hook: blocks one call when armed",
            ports: STALL_PORTS,
            parameters: &[],
            feedthrough: Feedthrough::Direct,
            sink: None,
            latency: None,
            reproducibility: signal_core::Reproducibility::BitExact,
            category: None,
        }
    }
    fn prepare(&mut self, _ctx: &PrepareContext) {}
    fn reset(&mut self) {}
    fn process(&mut self, inputs: &Inputs<'_>, outputs: &mut Outputs<'_>) {
        if ARMED.swap(false, Ordering::SeqCst) {
            let start = Instant::now();
            thread::sleep(3 * PERIOD);
            while !RELEASED.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(1));
            }
        }
        let input = inputs.signal(0);
        outputs.signal(0).copy_from_slice(input);
    }
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    signal_std::register(&mut registry).unwrap();
    registry
        .register(ModuleType {
            descriptor: Stall::describe(),
            create: || Primitive::Direct(Box::new(Stall)),
        })
        .unwrap();
    registry
}

/// A tone through `/stall` into the left channel, another tone straight
/// into the right: two independent tasks.
fn plan(registry: &Registry) -> ExecutionPlan {
    let instance = |name: &str, module: &str| InstanceDef {
        name: name.to_owned(),
        module: module.to_owned(),
        parameters: Default::default(),
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
            instance("tone", "std.sine_osc"),
            instance("stall", "test.stall"),
            instance("other", "std.sine_osc"),
            instance("out", "std.audio_out"),
        ],
        connections: vec![
            connection("tone.out", "stall.in"),
            connection("stall.out", "out.l"),
            connection("other.out", "out.r"),
        ],
        exports: Exports::default(),
    };
    build_plan(&root, &[], registry, 0).unwrap()
}

/// The hangs reported: (callback, waited, running instances).
#[derive(Default)]
struct Hangs(Vec<(u64, Duration, Vec<String>)>);

impl ControlSink for Hangs {
    fn hang(&mut self, line: HangLine<'_>) {
        self.0.push((
            line.callback,
            line.waited,
            line.running.iter().map(ToString::to_string).collect(),
        ));
    }
}

/// Plays `plan` on the threaded null driver with `threads` threads
/// (`forced`: every segment on the workers); with `stall`, one call of
/// `/stall` blocks. Returns the hangs reported, the Controller's count and
/// the parallel segments run.
fn play(threads: usize, forced: bool, stall: bool) -> (Hangs, u64, u64) {
    let registry = registry();
    let mut config = RealtimeConfig::with_buffer(RATE, 1, BUFFER);
    config.engine.threads = threads;
    if forced {
        config.engine.schedule = Schedule::FORCED;
    }
    config.hang_periods = HANG_PERIODS;
    let (runner, mut controller) = realtime(&plan(&registry), &registry, config).unwrap();
    RELEASED.store(false, Ordering::SeqCst);
    ARMED.store(stall, Ordering::SeqCst);
    let mut driver = NullDriver::new(RATE, 2, 3).threaded(false);
    driver.start(runner).unwrap();
    let mut hangs = Hangs::default();
    let start = Instant::now();
    // Until the hang is reported (with a stall), or for 20 periods.
    while start.elapsed() < Duration::from_secs(5) {
        controller.poll(&mut hangs);
        if !hangs.0.is_empty() || (!stall && start.elapsed() > 20 * PERIOD) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    RELEASED.store(true, Ordering::SeqCst);
    // The run goes on after the call returns: no second report.
    let after = Instant::now();
    while after.elapsed() < 4 * PERIOD {
        controller.poll(&mut hangs);
        thread::sleep(Duration::from_millis(1));
    }
    let runner = driver.stop().unwrap().unwrap();
    controller.poll(&mut hangs);
    assert!(runner.status().callbacks > 1, "{:?}", runner.status());
    (
        hangs,
        controller.hangs(),
        runner.schedule_stats().parallel_segments,
    )
}

fn assert_detected(hangs: &Hangs, count: u64) {
    assert_eq!(count, 1, "{:?}", hangs.0);
    let [(callback, waited, running)] = hangs.0.as_slice() else {
        panic!("one report expected: {:?}", hangs.0);
    };
    assert_eq!(*callback, 0, "the first callback stalled");
    assert!(*waited >= HANG_PERIODS * PERIOD, "{waited:?}");
    assert_eq!(running, &["/stall".to_owned()]);
}

/// One thread: the call runs on the calling thread alone.
#[test]
fn a_stalled_call_on_the_calling_thread_is_reported_with_its_instance() {
    let _serial = serial();
    let (hangs, count, parallel) = play(1, false, true);
    assert_eq!(parallel, 0);
    assert_detected(&hangs, count);
}

/// Two threads, every segment on the workers: the call runs in a task.
#[test]
fn a_stalled_call_in_a_parallel_segment_is_reported_with_its_instance() {
    let _serial = serial();
    let (hangs, count, parallel) = play(2, true, true);
    assert!(parallel > 0, "the segments ran on the workers");
    assert_detected(&hangs, count);
}

/// Without a stall nothing is reported.
#[test]
fn a_run_without_a_stall_reports_no_hang() {
    let _serial = serial();
    let (hangs, count, _) = play(2, false, false);
    assert_eq!(count, 0, "{:?}", hangs.0);
    assert!(hangs.0.is_empty());
}

/// The cases share the stall's flags: one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}
